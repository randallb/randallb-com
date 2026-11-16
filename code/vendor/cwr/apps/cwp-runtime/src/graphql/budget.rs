//! One deadline for a selection, including queueing and nested database reads.
use async_graphql::{
    extensions::{Extension, ExtensionContext, ExtensionFactory, NextExecute},
    Data, Error, ErrorExtensions, Response, Result,
};
use cwr::v2026_09::conversation::ConversationReadCancellation;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{watch, Semaphore};

#[derive(Clone)]
pub(super) struct ReadBudget(Arc<State>);
struct State {
    deadline: Instant,
    cancelled: AtomicBool,
    changed: watch::Sender<bool>,
    reads: Mutex<Vec<ConversationReadCancellation>>,
}
impl ReadBudget {
    pub fn new(duration: Duration) -> Self {
        Self(Arc::new(State {
            deadline: Instant::now() + duration,
            cancelled: AtomicBool::new(false),
            changed: watch::channel(false).0,
            reads: Mutex::new(Vec::new()),
        }))
    }
    pub fn deadline(&self) -> Instant {
        self.0.deadline
    }
    pub fn cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }
    pub fn check(&self) -> cwr::CwrResult<()> {
        if self.cancelled() {
            return Err(std::io::Error::other("read selection cancelled").into());
        }
        if Instant::now() >= self.deadline() {
            return Err(std::io::Error::other("read selection deadline exceeded").into());
        }
        Ok(())
    }
    pub async fn acquire(
        &self,
        permits: Arc<Semaphore>,
    ) -> Result<tokio::sync::OwnedSemaphorePermit> {
        let mut cancelled = self.0.changed.subscribe();
        self.check().map_err(|_| self.error())?;
        let permit = tokio::select! {
            permit = tokio::time::timeout_at(tokio::time::Instant::from_std(self.deadline()), permits.acquire_owned()) => permit.map_err(|_| self.error())?.map_err(|_| Error::new("Runtime workers are unavailable"))?,
            _ = cancelled.changed() => return Err(self.error()),
        };
        self.check().map_err(|_| self.error())?;
        Ok(permit)
    }
    pub fn register(&self, read: ConversationReadCancellation) {
        let mut reads = self.0.reads.lock().unwrap();
        if self.cancelled() {
            read.cancel();
        } else {
            reads.push(read);
        }
    }
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.changed.send_replace(true);
        for read in self.0.reads.lock().unwrap().drain(..) {
            read.cancel();
        }
    }
    fn error(&self) -> Error {
        let code = if Instant::now() >= self.deadline() {
            "READ_BUDGET_EXCEEDED"
        } else {
            "READ_CANCELLED"
        };
        Error::new("Runtime read selection exceeded its budget or was cancelled")
            .extend_with(|_, e| e.set("code", code))
    }
}
/// Cancels blocking work if its async owner is dropped. Never cancels admitted actions.
struct CancelOnDrop {
    budget: ReadBudget,
    armed: bool,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.budget.cancel();
        }
    }
}

pub(super) async fn blocking<T: Send + 'static>(
    permits: Arc<Semaphore>,
    budget: ReadBudget,
    read: impl FnOnce() -> cwr::CwrResult<T> + Send + 'static,
) -> Result<T> {
    let mut guard = CancelOnDrop {
        budget: budget.clone(),
        armed: true,
    };
    let mut cancelled = budget.0.changed.subscribe();
    let until = tokio::time::Instant::from_std(budget.deadline());
    let permit = budget.acquire(permits).await?;
    let operation = budget.clone();
    let task = tokio::task::spawn_blocking(move || {
        // The blocking task owns the permit until it actually stops, even if its
        // async waiter times out. Cancellation also covers waiting in the pool.
        let _permit = permit;
        operation.check()?;
        let value = read()?;
        operation.check()?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(value)
    });
    let result = tokio::select! {
        result = tokio::time::timeout_at(until, task) => result.map_err(|_|budget.error())?.map_err(|_|Error::new("Runtime read worker failed"))?,
        _ = cancelled.changed() => return Err(budget.error()),
    };
    guard.armed = false;
    result.map_err(|_| {
        if budget.check().is_err() {
            budget.error()
        } else {
            Error::new("Runtime resource read failed")
        }
    })
}

pub(super) struct BudgetExtension;
impl ExtensionFactory for BudgetExtension {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(Self)
    }
}
#[async_trait::async_trait]
impl Extension for BudgetExtension {
    async fn execute(
        &self,
        ctx: &ExtensionContext<'_>,
        operation_name: Option<&str>,
        next: NextExecute<'_>,
    ) -> Response {
        let Some(service) = ctx.data_opt::<super::schema::ReadService>() else {
            return next.run(ctx, operation_name).await;
        };
        let service = service.fresh_selection();
        let _guard = CancelOnDrop {
            budget: service.budget(),
            armed: true,
        };
        let mut data = Data::default();
        data.insert(service);
        next.run_with_data(ctx, operation_name, data).await
    }
}
