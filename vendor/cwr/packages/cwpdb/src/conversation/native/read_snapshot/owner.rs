use super::*;
// The owner temporarily lends its private connection to the same finite reader.
// No physical handle crosses the cwpdb boundary, including for in-memory stores.
struct OwnerReadLease<'a> {
    owner: &'a mut NativeConnection,
    pool: Arc<Mutex<Option<Connection>>>,
}
impl Drop for OwnerReadLease<'_> {
    fn drop(&mut self) {
        if let Ok(mut pool) = self.pool.lock() {
            if let Some(connection) = pool.take() {
                let _ = connection.progress_handler(0, None::<fn() -> bool>);
                let _ = connection.execute_batch("ROLLBACK");
                let _ = connection.pragma_update(None, "query_only", false);
                let _ = connection.busy_timeout(std::time::Duration::from_secs(5));
                self.owner.connection = Some(connection);
            }
        }
    }
}
pub(in crate::conversation::native) fn owner_selection(
    owner: &mut NativeConnection,
    request: &ReadSelectionRequest,
    control: &ReadControl,
) -> CwpdbResult<ReadSelectionFacts> {
    let bounded = request.bounded_control(control)?;
    let control = &bounded;
    let connection = owner
        .connection
        .take()
        .ok_or("native connection is already selected")?;
    let lease = OwnerReadLease {
        owner,
        pool: Arc::new(Mutex::new(Some(connection))),
    };
    {
        let available = lease.pool.lock().map_err(|_| "native read pool poisoned")?;
        let connection = available.as_ref().ok_or("missing native read connection")?;
        configure(connection, control)?;
        if !lease.owner.read_views {
            initialize_views(connection, control)?;
            lease.owner.read_views = true;
        }
        connection.pragma_update(None, "query_only", true)?;
    }
    let mut store = NativeReadStore {
        pool: lease.pool.clone(),
        scope: request.scope.clone(),
        identity: None,
    };
    let snapshot = store.begin_current(control.clone())?;
    snapshot.read_selection(request)
}

fn initialize_views(connection: &Connection, control: &ReadControl) -> CwpdbResult<()> {
    connection.execute_batch("SAVEPOINT finite_read_views")?;
    let result = (|| -> CwpdbResult<()> {
        connection.execute_batch(include_str!("read_views.sql"))?;
        control.check()?;
        connection.execute_batch("RELEASE finite_read_views")?;
        Ok(())
    })();
    if result.is_err() {
        // An interrupted rollback must not strand a partial temp-view schema.
        let _ = connection.progress_handler(0, None::<fn() -> bool>);
        let _ =
            connection.execute_batch("ROLLBACK TO finite_read_views; RELEASE finite_read_views");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_temp_view_setup_rolls_back_before_retry() {
        let connection = Connection::open_in_memory().unwrap();
        let control = ReadControl {
            cancellation: ReadCancellation::default(),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
            vm_steps: None,
        };
        let mut steps = 0;
        connection
            .progress_handler(
                1,
                Some(move || {
                    steps += 1;
                    steps >= 20
                }),
            )
            .unwrap();
        assert!(initialize_views(&connection, &control).is_err());
        // initialize_views clears interruption before rolling back its savepoint;
        // retry must not encounter any partially installed duplicate views.
        initialize_views(&connection, &control).unwrap();
    }
}
