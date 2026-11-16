use super::coordinator::{CloudCallback, CloudCoordinator};
use crate::CwrResult;
use cwpdb_store::invocation::{
    Authorizer, Context, Event, EventKind, InvocationStorage, Mutation, Operation, Outcome,
    ReceiptLookup,
};
use serde_json::json;

impl<S: InvocationStorage, A: Authorizer> CloudCoordinator<S, A> {
    /// Reserve process start before its external effect. A new coordinator or
    /// worker boot cannot replay an uncertain start, even on the same executor.
    /// The live boot keeps its own process handle and returns it on a duplicate.
    pub fn begin_execution(
        &mut self,
        context: Context,
        invocation_ref: String,
        token: &str,
        worker_boot_id: &str,
        now: u64,
    ) -> CwrResult<Outcome> {
        self.publication_authority()
            .authorize(&context, &invocation_ref, true)?;
        if worker_boot_id.trim().is_empty() {
            return Err("Executor boot identity is required".into());
        }
        let record = self.record(&context, &invocation_ref)?;
        self.verify(&record, token, now)?;
        if record.terminal.is_some() {
            return Err("Cloud invocation is terminal".into());
        }
        let assignment = record.assignment.ok_or("Cloud invocation is unassigned")?;
        let request_id =
            super::coordinator::internal_request_id("executor-start", &assignment.assignment_ref);
        let payload =
            json!({"lifecycle":"executor_start_reserved","worker_boot_id":worker_boot_id});
        let authority = self
            .publication_authorizer
            .as_ref()
            .unwrap_or(&self.authorizer);
        let retained = self
            .storage
            .receipt(&context, &invocation_ref, &request_id, authority)?;
        authority.authorize(&context, &invocation_ref, true)?;
        super::coordinator::executor_start_receipt_available(&retained)?;
        match retained {
            ReceiptLookup::Retained { request, receipt } => {
                if !matches!(request.operation,Operation::Append {event,..} if event.payload == payload)
                {
                    return Err(
                        "Prior executor start requires interruption and reconciliation".into(),
                    );
                }
                self.publication_authority()
                    .authorize(&context, &invocation_ref, true)?;
                return Ok(Outcome::Committed(receipt));
            }
            ReceiptLookup::CommittedResultExpired => {
                return Err(
                    "Executor start receipt expired; interruption and reconciliation required"
                        .into(),
                );
            }
            ReceiptLookup::NotCommitted => {}
        }
        if record.next_sequence != 0 {
            return Err("Executor start must precede output".into());
        }
        self.storage.mutate(
            &Mutation {
                context,
                invocation_ref,
                request_id,
                expected_revision: Some(record.revision),
                operation: Operation::Append {
                    assignment_ref: assignment.assignment_ref,
                    executor_ref: assignment.executor_ref,
                    generation: assignment.generation,
                    event: Event {
                        sequence: 0,
                        kind: EventKind::Progress,
                        payload,
                    },
                },
            },
            self.publication_authorizer
                .as_ref()
                .unwrap_or(&self.authorizer),
            now,
        )
    }

    /// Recover the exact original CAS precondition after acknowledgement loss.
    /// Conflicting frame/request reuse is rejected before replaying the receipt.
    pub fn publish_recorded(
        &mut self,
        context: Context,
        invocation_ref: String,
        token: &str,
        callback: CloudCallback,
        now: u64,
    ) -> CwrResult<Outcome> {
        self.publication_authority()
            .authorize(&context, &invocation_ref, true)?;
        let CloudCallback {
            request_id,
            sequence,
            frame,
        } = callback;
        let record = self.record(&context, &invocation_ref)?;
        self.verify(&record, token, now)?;
        let authority = self
            .publication_authorizer
            .as_ref()
            .unwrap_or(&self.authorizer);
        let retained = self
            .storage
            .receipt(&context, &invocation_ref, &request_id, authority)?;
        authority.authorize(&context, &invocation_ref, true)?;
        match retained {
            ReceiptLookup::Retained { request, .. } => {
                if !matches!(&request.operation,Operation::Append {event,..} if event.sequence == sequence && event.payload == frame)
                {
                    return Err(
                        "Cloud callback request identity was reused with different output".into(),
                    );
                }
                self.storage.mutate(&request, authority, now)
            }
            ReceiptLookup::CommittedResultExpired => Ok(Outcome::CommittedResultExpired),
            ReceiptLookup::NotCommitted => self.publish(
                context,
                invocation_ref,
                token,
                record.revision,
                CloudCallback {
                    request_id,
                    sequence,
                    frame,
                },
                now,
            ),
        }
    }
}
