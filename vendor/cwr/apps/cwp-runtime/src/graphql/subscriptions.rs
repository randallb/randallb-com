//! Snapshot subscriptions use the same authorized resource resolvers as queries.
use super::{authorize, schema, HttpState};
use async_graphql::{Context, Result};
use axum::{
    extract::{FromRequestParts, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures_util::{stream, Stream};
use std::sync::Arc;
use tokio::sync::{watch, Semaphore};

pub(super) struct Observation {
    pub changes: watch::Receiver<()>,
    pub restarts: watch::Receiver<Option<String>>,
    pub session_changes: watch::Receiver<()>,
    pub permits: Arc<Semaphore>,
}

fn selects_elapsed(field: async_graphql::SelectionField<'_>) -> bool {
    field.name() == "elapsedSeconds" || field.selection_set().any(selects_elapsed)
}

pub(super) struct Subscription;
#[async_graphql::Subscription]
impl Subscription {
    /// Emitted only for a committed development replacement, never an unexpected EOF.
    #[expect(
        clippy::unused_async,
        reason = "GraphQL subscription macro requires an async resolver"
    )]
    async fn runtime_restarting(&self, ctx: &Context<'_>) -> Result<impl Stream<Item = String>> {
        let observation = ctx.data_unchecked::<Observation>();
        let restarts = observation.restarts.clone();
        let permit = observation
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| async_graphql::Error::new("Subscription capacity exhausted"))?;
        Ok(stream::unfold(
            (restarts, false, permit),
            |(mut restarts, emitted, permit)| async move {
                if emitted {
                    return None;
                }
                loop {
                    let generation = restarts.borrow_and_update().clone();
                    if let Some(generation) = generation {
                        return Some((generation, (restarts, true, permit)));
                    }
                    restarts.changed().await.ok()?;
                }
            },
        ))
    }

    /// Initial authoritative selection, then fresh selections after runtime publications.
    /// Intermediate publications may be coalesced. Reconnect to obtain current state.
    #[expect(
        clippy::unused_async,
        reason = "GraphQL subscription macro requires an async resolver"
    )]
    async fn runtime(
        &self,
        ctx: &Context<'_>,
    ) -> Result<impl Stream<Item = Result<schema::Runtime>>> {
        let service = ctx.data_unchecked::<schema::ReadService>().clone();
        let timing = ctx.field().selection_set().any(selects_elapsed);
        let observation = ctx.data_unchecked::<Observation>();
        let permit = observation
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| async_graphql::Error::new("Subscription capacity exhausted"))?;
        // Register before reading the first snapshot so a racing publication is never lost.
        let changes = observation.changes.clone();
        let session_changes = observation.session_changes.clone();
        Ok(stream::unfold(
            (service, changes, session_changes, true, permit, timing),
            |(service, mut changes, mut session_changes, first, permit, timing)| async move {
                if !first {
                    let changed = tokio::select! {
                        r = changes.changed() => r,
                        r = session_changes.changed() => r,
                        _ = tokio::time::sleep(std::time::Duration::from_millis(500)), if timing => Ok(()),
                    };
                    if changed.is_err() {
                        return None;
                    }
                }
                changes.borrow_and_update();
                session_changes.borrow_and_update();
                let selection = service.fresh_selection();
                let check = selection.clone();
                let allowed =
                    schema::blocking(selection.reads.clone(), selection.budget(), move || {
                        check.catalog().map(|_| ())
                    })
                    .await;
                let value = allowed.map(|_| schema::Runtime(selection));
                Some((
                    value,
                    (service, changes, session_changes, false, permit, timing),
                ))
            },
        ))
    }
    #[expect(
        clippy::unused_async,
        reason = "GraphQL subscription macro requires an async resolver"
    )]
    async fn terminal_events(
        &self,
        ctx: &Context<'_>,
    ) -> Result<impl Stream<Item = super::terminals::TerminalEvent>> {
        let session = ctx
            .data_unchecked::<Arc<super::sessions::ClientSession>>()
            .clone();
        let receiver = session.listen().map_err(|_| {
            async_graphql::Error::new(
                "Terminal events require one live session connection and one subscriber",
            )
        })?;
        let ready = super::terminals::TerminalEvent::Ready(super::terminals::SessionReady {
            session_ref: async_graphql::ID(session.id.clone()),
        });
        let guard = super::sessions::SessionGuard(session);
        Ok(stream::unfold(
            (receiver, Some(ready), guard),
            |(mut receiver, ready, guard)| async move {
                let event = match ready {
                    Some(ready) => ready,
                    None => receiver.recv().await?.into(),
                };
                Some((event, (receiver, None, guard)))
            },
        ))
    }
}

pub(super) async fn upgrade(
    State(state): State<HttpState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    let session = authorize(&state, &headers)?;
    if !headers
        .get_all("sec-websocket-protocol")
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(','))
        .any(|offer| offer.trim() == "graphql-transport-ws")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Use the same selected protocol for the HTTP upgrade and GraphQL framing,
    // even when the client lists a legacy fallback first.
    let (mut selected, _) = axum::http::Request::new(()).into_parts();
    selected.headers.insert(
        "sec-websocket-protocol",
        "graphql-transport-ws".parse().unwrap(),
    );
    let protocol =
        async_graphql_axum::GraphQLProtocol::from_request_parts(&mut selected, &()).await?;
    let permit = state
        .sockets
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let consumer = state
        .activity
        .connect()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    session.attach().map_err(|_| StatusCode::CONFLICT)?;
    let guard = super::sessions::SessionGuard(session.clone());
    Ok(ws
        .protocols(["graphql-transport-ws"])
        .max_message_size(64 * 1024)
        .max_frame_size(64 * 1024)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            let _permit = permit;
            let _consumer = consumer;
            let mut shutdown = state.shutdown.clone();
            if *shutdown.borrow() || !session.alive() {
                return;
            }
            let mut session_shutdown = session.shutdown.subscribe();
            let mut data = async_graphql::Data::default();
            data.insert(session.service.clone());
            data.insert(session);
            let connection =
                async_graphql_axum::GraphQLWebSocket::new(socket, state.schema, protocol)
                    .with_data(data)
                    .keepalive_timeout(std::time::Duration::from_secs(30))
                    .serve();
            tokio::select! {
                // Closing must win over a ready snapshot: its blocking reader
                // may already be cancelled by runtime shutdown.
                biased;
                _ = shutdown.changed() => {},
                _ = session_shutdown.changed() => {},
                _ = connection => {},
            }
        }))
}
