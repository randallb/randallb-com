//! Activity durations use retained start/completion evidence and the snapshot time.
use super::messages::{datetime, DateTime};
use async_graphql::{Interface, SimpleObject};

#[derive(Clone, SimpleObject)]
pub(super) struct RunningActivityTiming {
    #[graphql(owned)]
    started_at: DateTime,
    #[graphql(owned)]
    finished_at: Option<DateTime>,
    #[graphql(owned)]
    elapsed_seconds: f64,
    #[graphql(owned)]
    measured_at: DateTime,
}
#[derive(Clone, SimpleObject)]
pub(super) struct CompletedActivityTiming {
    #[graphql(owned)]
    started_at: DateTime,
    #[graphql(owned)]
    finished_at: DateTime,
    #[graphql(owned)]
    elapsed_seconds: f64,
    #[graphql(owned)]
    measured_at: DateTime,
}
#[derive(Clone, Interface)]
#[expect(
    clippy::duplicated_attributes,
    reason = "GraphQL timing fields share scalar types"
)]
#[graphql(
    field(name = "started_at", ty = "DateTime"),
    field(name = "finished_at", ty = "Option<DateTime>"),
    field(name = "elapsed_seconds", ty = "f64"),
    field(name = "measured_at", ty = "DateTime")
)]
pub(super) enum ActivityTiming {
    Running(RunningActivityTiming),
    Completed(CompletedActivityTiming),
}
pub(super) fn project(
    timing: cwr::v2026_09::conversation::ConversationActivityTiming,
) -> cwr::CwrResult<ActivityTiming> {
    let started_at = datetime(i128::from(timing.started_at_unix_ms))?;
    let measured_at = datetime(i128::from(timing.measured_at_unix_ms))?;
    Ok(match timing.finished_at_unix_ms {
        Some(finished) => ActivityTiming::Completed(CompletedActivityTiming {
            started_at,
            finished_at: datetime(i128::from(finished))?,
            elapsed_seconds: timing.elapsed_seconds,
            measured_at,
        }),
        None => ActivityTiming::Running(RunningActivityTiming {
            started_at,
            finished_at: None,
            elapsed_seconds: timing.elapsed_seconds,
            measured_at,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};
    struct Query;
    #[Object]
    impl Query {
        async fn timing(&self) -> ActivityTiming {
            project(cwr::v2026_09::conversation::ConversationActivityTiming {
                started_at_unix_ms: 1000,
                finished_at_unix_ms: Some(2250),
                elapsed_seconds: 1.25,
                measured_at_unix_ms: 2250,
            })
            .unwrap()
        }
    }
    #[tokio::test]
    async fn completed_timing_has_fixed_duration_and_narrows_finished_at() {
        let schema = Schema::new(Query, EmptyMutation, EmptySubscription);
        let response=schema.execute("{timing{__typename elapsedSeconds startedAt measuredAt ...on CompletedActivityTiming{finishedAt}}}").await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        assert_eq!(data["timing"]["__typename"], "CompletedActivityTiming");
        assert_eq!(data["timing"]["elapsedSeconds"], 1.25);
        assert_eq!(data["timing"]["finishedAt"], "1970-01-01T00:00:02.250Z");
        assert_eq!(data["timing"]["finishedAt"], data["timing"]["measuredAt"]);
    }
}
