use cwpdb::{
    declared::{Catalog, EntityContract, ValueValidator},
    schedules::*,
    CwpdbResult,
};
use serde_json::{json, Value};
struct Validator;
impl ValueValidator for Validator {
    fn validate_reference(&self, reference: &str) -> CwpdbResult<()> {
        if !reference.starts_with("cwp://") {
            return Err("fixture canonical ref required".into());
        }
        Ok(())
    }
    fn validate(&self, _: &str, properties: &Value) -> CwpdbResult<()> {
        if !properties["record"].is_object() {
            return Err("fixture record object required".into());
        }
        Ok(())
    }
}
fn catalog() -> CwpdbResult<Catalog> {
    Catalog::new(
        [
            ("installed-schedule", "installed_schedule"),
            ("schedule-occurrence", "schedule_occurrence"),
            ("schedule-owner", "schedule_owner"),
        ]
        .into_iter()
        .map(|(name, kind)| EntityContract {
            definition_ref: format!("cwp://org.coworkerprotocol/cwr/entities/{name}"),
            kind: kind.into(),
            schema_ref: format!("cwp://org.coworkerprotocol/cwpdb/schemas/{kind}/2026-09"),
            disposable: false,
        })
        .collect(),
        std::sync::Arc::new(Validator),
    )
}
pub fn dispatch(
    path: &std::path::Path,
    endpoint: &Option<String>,
    body: &Value,
) -> CwpdbResult<Value> {
    let mut store: Box<dyn ScheduleStorage> = if let Some(endpoint) = endpoint {
        Box::new(RemoteSchedules::new(
            super::FixtureTransport {
                endpoint: endpoint.clone(),
            },
            serde_json::from_value(body["context"].clone())?,
        ))
    } else {
        Box::new(cwpdb::NativeStore::open(path)?)
    };
    let catalog = catalog()?;
    let r = &body["schedule_request"];
    let reference = || r["reference"].as_str().ok_or("fixture reference required");
    let owner = r["owner"].as_str();
    let root = || {
        r["runtime_root_ref"]
            .as_str()
            .ok_or("fixture root required")
    };
    match r["action"].as_str().ok_or("fixture action required")? {
        "installed" => Ok(serde_json::to_value(store.installed(
            &catalog,
            root()?,
            owner,
        )?)?),
        "get_schedule" => Ok(serde_json::to_value(store.get_schedule(
            &catalog,
            reference()?,
            owner,
        )?)?),
        "get_occurrence" => Ok(serde_json::to_value(
            store.get_occurrence(&catalog, reference()?)?,
        )?),
        "occurrences" => Ok(serde_json::to_value(store.occurrences(
            &catalog,
            reference()?,
            serde_json::from_value(r["selection"].clone())?,
        )?)?),
        "archive_candidates" => Ok(serde_json::to_value(
            store.archive_candidates(&catalog, root()?)?,
        )?),
        "install" => Ok(serde_json::to_value(
            store.install(&catalog, &serde_json::from_value(r["request"].clone())?)?,
        )?),
        "pause" => Ok(serde_json::to_value(store.pause(
            &catalog,
            owner.ok_or("owner required")?,
            reference()?,
            &serde_json::from_value(r["expected"].clone())?,
            r["paused"].as_bool().ok_or("paused required")?,
            r["next_due_unix_ms"].as_u64().ok_or("time required")?,
        )?)?),
        "availability" => {
            store.availability(
                &catalog,
                owner.ok_or("owner required")?,
                &serde_json::from_value::<Vec<Availability>>(r["changes"].clone())?,
            )?;
            Ok(json!(null))
        }
        "advance" => Ok(serde_json::to_value(
            store.advance(&catalog, &serde_json::from_value(r["request"].clone())?)?,
        )?),
        "reserve" => Ok(serde_json::to_value(
            store.reserve(&catalog, &serde_json::from_value(r["request"].clone())?)?,
        )?),
        "settle" => Ok(serde_json::to_value(
            store.settle(&catalog, &serde_json::from_value(r["request"].clone())?)?,
        )?),
        _ => Err("unsupported fixture schedule operation".into()),
    }
}
