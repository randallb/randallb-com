//! CWR-owned schema validation for the protected cwpdb telemetry outbox.
use super::EntityCatalog;
use crate::{cwpdb, CwrResult};
pub use cwpdb::posthog::{
    MigrationReport, PendingEvent, Selection, EVENT_DEFINITION, STATE_DEFINITION,
};
use std::{path::Path, sync::Arc};
pub struct PosthogOutbox(cwpdb::posthog::PosthogOutbox);
impl PosthogOutbox {
    pub fn open(directory: &Path) -> CwrResult<Self> {
        let entities = Arc::new(EntityCatalog::from_toml_documents([
            include_str!("../../resources/base/entities/posthog-event.toml"),
            include_str!("../../resources/base/entities/posthog-state.toml"),
        ])?);
        let contracts = entities
            .types()
            .into_iter()
            .map(|definition| entities.contract(definition))
            .collect::<CwrResult<Vec<_>>>()?;
        let catalog = cwpdb::declared::Catalog::new(contracts, entities)?;
        Ok(Self(cwpdb::posthog::PosthogOutbox::open(
            directory, &catalog,
        )?))
    }
}
impl std::ops::Deref for PosthogOutbox {
    type Target = cwpdb::posthog::PosthogOutbox;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for PosthogOutbox {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
