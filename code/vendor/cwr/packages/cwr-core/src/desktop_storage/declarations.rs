//! Compile the CWR-owned declarations for named Desktop storage operations.
use crate::{cwpdb::declared::Catalog, entities::EntityCatalog, CwrResult};
use std::sync::{Arc, OnceLock};
pub(super) fn catalog() -> CwrResult<&'static Catalog> {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    if CATALOG.get().is_none() {
        let entities = Arc::new(EntityCatalog::from_toml_documents([
            include_str!("../../resources/base/entities/personal-collection.toml"),
            include_str!("../../resources/base/entities/personal-collection-scope.toml"),
            include_str!("../../resources/base/entities/sidebar-pin-target.toml"),
            include_str!("../../resources/base/entities/sidebar-pin-scope.toml"),
            include_str!("../../resources/base/entities/local-usage-cursor.toml"),
            include_str!("../../resources/base/entities/local-usage-event.toml"),
            include_str!("../../resources/base/entities/variable.toml"),
            include_str!("../../resources/base/entities/desktop-workspace.toml"),
            include_str!("../../resources/base/entities/desktop-workspace-binding.toml"),
            include_str!("../../resources/base/entities/desktop-registry-receipt.toml"),
            include_str!("../../resources/base/entities/desktop-instance.toml"),
            include_str!("../../resources/base/entities/desktop-version.toml"),
            include_str!("../../resources/base/entities/desktop-folder.toml"),
            include_str!("../../resources/base/entities/desktop-version-membership.toml"),
            include_str!("../../resources/base/entities/desktop-import.toml"),
        ])?);
        let contracts = entities
            .types()
            .into_iter()
            .map(|definition| entities.contract(definition))
            .collect::<CwrResult<Vec<_>>>()?;
        let _ = CATALOG.set(Catalog::new(contracts, entities)?);
    }
    Ok(CATALOG
        .get()
        .expect("successful Desktop declaration compilation"))
}
