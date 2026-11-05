//! Build a finite read selection after releasing the LocalRuntime state lock.
use super::super::backend::ConversationBackend;
use super::*;
use cwpdb_store::conversation::{
    ReadControl, ReadSelectionFacts, ReadSelectionRequest, ReadSelectionSource, ReadSnapshot, Scope,
};
use std::sync::{Arc, Mutex, TryLockError};

pub(crate) struct ReadStoreLocation {
    backend: Arc<Mutex<ConversationBackend>>,
    path: PathBuf,
    runtime_root_ref: String,
}
impl ReadStoreLocation {
    pub(in crate::v2026_09::conversation) fn new(
        backend: Arc<Mutex<ConversationBackend>>,
        path: PathBuf,
        runtime_root_ref: String,
    ) -> Self {
        Self {
            backend,
            path,
            runtime_root_ref,
        }
    }
    pub(crate) fn open_until(
        self,
        deadline: Instant,
        cancellation: &ConversationReadCancellation,
    ) -> CwrResult<ConversationReadStore> {
        let control = cancellation.storage_control_for_path(deadline, &self.path);
        let (native, scope) = {
            let backend = backend_lock(&self.backend, &control)?;
            (backend.is_native(), backend.scope.clone())
        };
        if native {
            return ConversationReadStore::open_until(
                self.path,
                &self.runtime_root_ref,
                deadline,
                cancellation,
            );
        }
        Ok(ConversationReadStore {
            path: self.path,
            factory: ReadFactory::Remote {
                scope,
                backend: self.backend,
            },
        })
    }
}
pub(super) enum ReadFactory {
    Native(cwpdb_store::conversation::NativeReadStore),
    Remote {
        scope: Scope,
        backend: Arc<Mutex<ConversationBackend>>,
    },
}
impl ReadFactory {
    pub(super) fn begin(&mut self, control: ReadControl) -> CwrResult<ReadSnapshot> {
        match self {
            Self::Native(store) => Ok(store.begin_current(control)?.into()),
            Self::Remote { scope, backend } => ReadSnapshot::remote(
                scope.clone(),
                control,
                Box::new(BackendReadSource {
                    backend: backend.clone(),
                }),
            ),
        }
    }
}
struct BackendReadSource {
    backend: Arc<Mutex<ConversationBackend>>,
}
impl ReadSelectionSource for BackendReadSource {
    fn read_selection(
        &mut self,
        request: &ReadSelectionRequest,
        control: &ReadControl,
    ) -> cwpdb_store::CwpdbResult<ReadSelectionFacts> {
        let mut backend = backend_lock(&self.backend, control)?;
        if request.scope != backend.scope {
            return invalid("read selection workspace or caller mismatch");
        }
        let ConversationBackend {
            storage,
            authorizer,
            ..
        } = &mut *backend;
        storage.read_selection(request, control, authorizer.as_ref())
    }
}
fn backend_lock<'a>(
    backend: &'a Mutex<ConversationBackend>,
    control: &ReadControl,
) -> CwrResult<std::sync::MutexGuard<'a, ConversationBackend>> {
    loop {
        control.check()?;
        match backend.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
            Err(TryLockError::Poisoned(_)) => return invalid("conversation backend lock poisoned"),
        }
    }
}
