//! Native subscription client. Each result is one JSON line for terminal tools.
use cwr::v2026_09::local::LocalGraphqlEndpoint;
use serde_json::{json, Value};
use std::{
    error::Error,
    io::{self, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};
use tungstenite::{client::IntoClientRequest, Message};

pub(super) fn run(
    endpoint: &LocalGraphqlEndpoint,
    query: &str,
    variables: Value,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut request = endpoint
        .url()
        .replacen("http:", "ws:", 1)
        .into_client_request()?;
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", endpoint.token).parse()?,
    );
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "graphql-transport-ws".parse()?);
    let stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    // Do not include the handshake request (which contains a credential) in errors.
    let (mut socket, _) =
        tungstenite::client(request, stream).map_err(|_| "GraphQL WebSocket handshake failed")?;
    socket.send(Message::Text(
        json!({"type":"connection_init"}).to_string().into(),
    ))?;
    consume(
        socket,
        query,
        variables,
        Duration::from_secs(10),
        &mut io::stdout().lock(),
    )
}

fn consume(
    mut socket: tungstenite::WebSocket<UnixStream>,
    query: &str,
    variables: Value,
    heartbeat: Duration,
    output: &mut impl Write,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut initialized = false;
    let mut next_ping = Instant::now() + heartbeat;
    loop {
        if initialized {
            let now = Instant::now();
            if now >= next_ping {
                socket.send(Message::Text(json!({"type":"ping"}).to_string().into()))?;
                next_ping = Instant::now() + heartbeat;
            }
            // Incoming snapshots never reset the heartbeat deadline. Wake at
            // that deadline even when the stream becomes idle between updates.
            socket.get_mut().set_read_timeout(Some(
                next_ping
                    .saturating_duration_since(Instant::now())
                    .max(Duration::from_millis(1)),
            ))?;
        }
        let message = match socket.read() {
            Ok(message) => message,
            Err(tungstenite::Error::Io(error))
                if initialized
                    && matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
            {
                continue;
            }
            Err(_) => {
                return Err(
                    "GraphQL subscription disconnected; run again for a fresh snapshot".into(),
                )
            }
        };
        match message {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(text.as_str())?;
                match value["type"].as_str() {
                    Some("connection_ack") if !initialized => {
                        initialized = true;
                        socket.send(Message::Text(json!({"type":"subscribe","id":"cli","payload":{"query":query,"variables":variables}}).to_string().into()))?;
                    }
                    Some("next") => {
                        let payload = &value["payload"];
                        writeln!(output, "{}", serde_json::to_string(payload)?)?;
                        output.flush()?;
                        if payload.get("errors").is_some() {
                            return Err("GraphQL subscription returned errors".into());
                        }
                    }
                    Some("error") => return Err("GraphQL subscription rejected".into()),
                    Some("complete") => return Ok(()),
                    Some("ping") => socket.send(Message::Text(
                        json!({"type":"pong","payload":value.get("payload")})
                            .to_string()
                            .into(),
                    ))?,
                    Some("pong") => {}
                    _ => return Err("Unexpected GraphQL WebSocket message".into()),
                }
            }
            Message::Ping(_) => socket.flush()?,
            Message::Close(_) => {
                return Err("GraphQL subscription closed; run again for a fresh snapshot".into())
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn heartbeats_follow_elapsed_time_during_idle_and_continuous_updates() {
        for busy in [false, true] {
            let (client, server) = UnixStream::pair().unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            server
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let server = std::thread::spawn(move || {
                let mut socket = tungstenite::accept(server).unwrap();
                socket
                    .send(Message::Text(
                        json!({"type":"connection_ack"}).to_string().into(),
                    ))
                    .unwrap();
                assert!(matches!(socket.read().unwrap(), Message::Text(_)));
                socket
                    .get_mut()
                    .set_read_timeout(Some(Duration::from_millis(5)))
                    .unwrap();
                let until = Instant::now() + Duration::from_millis(600);
                let mut pings = 0;
                while Instant::now() < until {
                    if busy {
                        socket
                            .send(Message::Text(
                                json!({"type":"next","payload":{"data":{}}})
                                    .to_string()
                                    .into(),
                            ))
                            .unwrap();
                    }
                    match socket.read() {
                        Ok(Message::Text(text)) => {
                            let value: Value = serde_json::from_str(&text).unwrap();
                            assert_eq!(value["type"], "ping");
                            pings += 1;
                        }
                        Err(tungstenite::Error::Io(e))
                            if matches!(
                                e.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                            ) => {}
                        other => panic!("unexpected {other:?}"),
                    }
                }
                socket
                    .send(Message::Text(json!({"type":"complete"}).to_string().into()))
                    .unwrap();
                pings
            });
            let (socket, _) = tungstenite::client("ws://localhost/graphql", client).unwrap();
            let mut output = Vec::new();
            consume(
                socket,
                "subscription{runtime{id}}",
                json!({}),
                Duration::from_millis(100),
                &mut output,
            )
            .unwrap();
            assert!(
                server.join().unwrap() >= 3,
                "heartbeat stopped with busy={busy}"
            );
            assert_eq!(!output.is_empty(), busy);
        }
    }
}
