use super::{invocations::ActionInvocation, schema::ReadService};
use async_graphql::{Enum, SimpleObject};

#[derive(Clone, Copy, PartialEq, Eq, Enum)]
pub(super) enum StopRequestDisposition {
    Pending,
    Accepted,
    TooLate,
    Rejected,
}

#[derive(Clone, SimpleObject)]
pub(super) struct StopControl {
    pub target: Option<ActionInvocation>,
    pub disposition: StopRequestDisposition,
}

pub(super) async fn project(
    service: ReadService,
    reference: String,
) -> async_graphql::Result<Option<StopControl>> {
    super::schema::blocking(service.reads.clone(), service.budget(), move || {
        use cwr::v2026_09::conversation::InvocationStopDisposition as Disposition;
        let Some(control) = service
            .publications
            .read(&service, |view| view.invocation_stop_control(&reference))?
        else {
            return Ok(None);
        };
        Ok(Some(StopControl {
            target: control
                .target
                .map(|target| super::invocations::project(target, service.clone()))
                .transpose()?,
            disposition: match control.disposition {
                Disposition::Pending => StopRequestDisposition::Pending,
                Disposition::Accepted => StopRequestDisposition::Accepted,
                Disposition::TooLate => StopRequestDisposition::TooLate,
                Disposition::Rejected => StopRequestDisposition::Rejected,
            },
        }))
    })
    .await
}
