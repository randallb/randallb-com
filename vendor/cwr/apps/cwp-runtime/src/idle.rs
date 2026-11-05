use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const IDLE_GRACE: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct Activity(
    Arc<Mutex<State>>,
    tokio::sync::watch::Sender<Option<String>>,
);
impl Default for Activity {
    fn default() -> Self {
        Self(Arc::default(), tokio::sync::watch::channel(None).0)
    }
}

#[derive(Default)]
struct State {
    consumers: usize,
    deliveries: usize,
    idle_since: Option<Instant>,
    exit_when_idle: bool,
    closing: bool,
    restart_at: Option<Instant>,
}

pub(crate) struct Consumer(Activity);
pub(crate) struct Delivery(Activity);

impl Drop for Delivery {
    fn drop(&mut self) {
        self.0 .0.lock().unwrap().deliveries -= 1;
    }
}

impl Activity {
    pub(crate) fn delivery(&self) -> Delivery {
        self.0.lock().unwrap().deliveries += 1;
        Delivery(self.clone())
    }

    pub(crate) fn connect(&self) -> Option<Consumer> {
        let mut state = self.0.lock().unwrap();
        if state.closing || state.restart_at.is_some() {
            return None;
        }
        state.consumers += 1;
        state.idle_since = None;
        Some(Consumer(self.clone()))
    }

    pub(crate) fn should_exit(
        &self,
        auto_exit: bool,
        has_work: impl FnOnce() -> cwr::CwrResult<bool>,
    ) -> cwr::CwrResult<bool> {
        self.check_at(Instant::now(), auto_exit, has_work)
    }

    pub(crate) fn exit_when_idle(&self) {
        let mut state = self.0.lock().unwrap();
        state.exit_when_idle = true;
        state.idle_since = None;
    }

    pub(crate) fn restart_notifications(&self) -> tokio::sync::watch::Receiver<Option<String>> {
        self.1.subscribe()
    }

    pub(crate) fn retire_after_disconnect(&self) {
        self.0.lock().unwrap().restart_at = Some(Instant::now());
    }

    pub(crate) fn restart_after_disconnect(&self, generation: &str) {
        let mut state = self.0.lock().unwrap();
        state.restart_at = Some(Instant::now());
        self.1.send_replace(Some(generation.into()));
    }

    fn check_at(
        &self,
        now: Instant,
        auto_exit: bool,
        has_work: impl FnOnce() -> cwr::CwrResult<bool>,
    ) -> cwr::CwrResult<bool> {
        // Serialize the final idle decision with both listeners accepting clients.
        let mut state = self.0.lock().unwrap();
        if let Some(started) = state.restart_at {
            // All execution has already drained. Allow host bridges to forward the
            // notice. Socket writers enforce their own stall timeout; exiting
            // earlier would truncate an otherwise healthy snapshot or notice.
            if state.deliveries == 0
                && (state.consumers == 0 || now.duration_since(started) >= Duration::from_secs(6))
            {
                state.closing = true;
                return Ok(true);
            }
            return Ok(false);
        }
        // Communication adapters keep an idle service alive, but must not
        // suppress an explicitly committed development restart.
        if !auto_exit {
            return Ok(false);
        }
        if state.consumers != 0 || has_work()? {
            state.idle_since = None;
            return Ok(false);
        }
        if state.exit_when_idle {
            state.closing = true;
            return Ok(true);
        }
        let since = *state.idle_since.get_or_insert(now);
        if now.duration_since(since) < IDLE_GRACE {
            return Ok(false);
        }
        state.closing = true;
        Ok(true)
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        self.0 .0.lock().unwrap().consumers -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_retirement_exits_without_development_generation_notification() {
        let activity = Activity::default();
        let notification = activity.restart_notifications();
        let consumer = activity.connect().unwrap();
        activity.retire_after_disconnect();
        assert!(notification.borrow().is_none());
        assert!(activity.connect().is_none());
        assert!(!activity.should_exit(false, || Ok(false)).unwrap());
        drop(consumer);
        assert!(activity.should_exit(false, || Ok(false)).unwrap());
    }

    #[test]
    fn idle_exit_waits_for_all_consumers_and_work_then_fences_connections() {
        let activity = Activity::default();
        let start = Instant::now();
        let first = activity.connect().unwrap();
        let second = activity.connect().unwrap();
        drop(first);
        assert!(!activity.check_at(start, true, || Ok(false)).unwrap());
        drop(second);
        assert!(!activity.check_at(start, true, || Ok(true)).unwrap());
        let idle = start + IDLE_GRACE;
        assert!(!activity.check_at(idle, true, || Ok(false)).unwrap());
        assert!(!activity
            .check_at(idle + IDLE_GRACE / 2, true, || Ok(false))
            .unwrap());
        assert!(activity
            .check_at(idle + IDLE_GRACE, true, || Ok(false))
            .unwrap());
        assert!(activity.connect().is_none());
    }

    #[test]
    fn reconnect_and_new_work_reset_the_grace_period() {
        let activity = Activity::default();
        let start = Instant::now();
        assert!(!activity.check_at(start, true, || Ok(false)).unwrap());
        drop(activity.connect().unwrap());
        assert!(!activity
            .check_at(start + IDLE_GRACE, true, || Ok(false))
            .unwrap());
        assert!(!activity
            .check_at(start + IDLE_GRACE * 2, true, || Ok(true))
            .unwrap());
        assert!(!activity
            .check_at(start + IDLE_GRACE * 3, true, || Ok(false))
            .unwrap());
        assert!(activity
            .check_at(start + IDLE_GRACE * 4, true, || Ok(false))
            .unwrap());
    }

    #[test]
    fn explicit_exit_skips_grace_after_the_last_consumer_leaves() {
        let activity = Activity::default();
        let consumer = activity.connect().unwrap();
        activity.exit_when_idle();
        assert!(!activity.should_exit(true, || Ok(false)).unwrap());
        drop(consumer);
        assert!(activity.should_exit(true, || Ok(false)).unwrap());
        assert!(activity.connect().is_none());
    }

    #[test]
    fn disabled_idle_exit_keeps_the_service_available_after_grace() {
        let activity = Activity::default();
        let start = Instant::now();
        for now in [start, start + IDLE_GRACE * 2] {
            assert!(!activity
                .check_at(now, false, || panic!("idle exit disabled"))
                .unwrap());
        }
        assert!(activity.connect().is_some());
    }

    #[test]
    fn planned_restart_fences_connections_and_bounds_the_notice_delivery_wait() {
        let activity = Activity::default();
        let consumer = activity.connect().unwrap();
        let delivery = activity.delivery();
        activity.restart_after_disconnect("test-generation");
        assert!(activity.connect().is_none());
        assert!(!activity
            .should_exit(false, || panic!("execution was already drained"))
            .unwrap());
        assert!(!activity
            .check_at(Instant::now() + Duration::from_secs(7), false, || Ok(true))
            .unwrap());
        drop(delivery);
        assert!(activity
            .check_at(Instant::now() + Duration::from_secs(7), false, || Ok(true))
            .unwrap());
        drop(consumer);

        let activity = Activity::default();
        activity.restart_after_disconnect("test-generation");
        assert!(
            activity.should_exit(false, || Ok(true)).unwrap(),
            "queued work is preserved for the replacement"
        );
    }
}
