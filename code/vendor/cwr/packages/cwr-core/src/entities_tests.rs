use super::*;
use serde_json::json;

const TYPE: &str = "cwp://org.coworkerprotocol/cwr/entities/variable";
const REF: &str = "cwp://com.example/entities/one";
fn catalog() -> EntityCatalog {
    EntityCatalog::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/base/CWR.toml"))
        .unwrap()
}

#[test]
fn composes_base_and_creates_validates_and_reopens() {
    let graph_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/base/CWR.toml");
    let graph = resolve_graph(&graph_path).unwrap();
    assert_eq!(
        graph
            .resources
            .iter()
            .find(|r| r.reference == TYPE)
            .unwrap()
            .reference,
        TYPE
    );
    let catalog = catalog();
    assert_eq!(
        catalog.types(),
        [
            "cwp://org.coworkerprotocol/cwr/entities/conversation",
            "cwp://org.coworkerprotocol/cwr/entities/conversation-activity",
            "cwp://org.coworkerprotocol/cwr/entities/conversation-message",
            "cwp://org.coworkerprotocol/cwr/entities/conversation-participation",
            "cwp://org.coworkerprotocol/cwr/entities/conversation-timeline-connection",
            "cwp://org.coworkerprotocol/cwr/entities/conversation-timeline-item",
            "cwp://org.coworkerprotocol/cwr/entities/conversation-transfer-metadata",
            TYPE,
            "cwp://org.coworkerprotocol/wlp/entities/workspace-secret",
            "cwp://org.coworkerprotocol/wlp/entities/workspace-variable"
        ]
    );
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.sqlite");
    let mut store = NativeStore::open(&path).unwrap();
    let value = json!({"name":"EXAMPLE", "value":"fictional"});
    let first = catalog.create(&mut store, TYPE, REF, &value).unwrap();
    assert_eq!(first.value.properties, value);
    assert!(catalog
        .create(&mut store, TYPE, REF, &value)
        .unwrap_err()
        .to_string()
        .contains("already_exists"));
    assert!(catalog
        .create(
            &mut store,
            TYPE,
            "cwp://com.example/entities/invalid",
            &json!({"name":"bad name","value":"x"})
        )
        .is_err());
    assert!(store
        .get_entity("cwp://com.example/entities/invalid")
        .is_err());
    assert!(catalog
        .create(&mut store, TYPE, "~@/relative", &value)
        .is_err());
    assert!(catalog
        .create(&mut store, TYPE, "cwp://com.example/entity#member", &value)
        .is_err());
    assert!(catalog.create(&mut store, TYPE, TYPE, &value).is_err());
    drop(store);
    let store = NativeStore::open_read_only(path).unwrap();
    assert_eq!(catalog.get(&store, TYPE, REF).unwrap(), first);
    assert!(catalog
        .get(&store, "cwp://com.example/unknown", REF)
        .is_err());
}

#[test]
fn independent_competing_creates_have_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.sqlite");
    drop(NativeStore::open(&path).unwrap());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let catalog = catalog();
                let mut store = NativeStore::open(path).unwrap();
                barrier.wait();
                catalog
                    .create(
                        &mut store,
                        TYPE,
                        REF,
                        &json!({"name":"RACE","value":i.to_string()}),
                    )
                    .is_ok()
            })
        })
        .collect();
    assert_eq!(
        handles
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum::<usize>(),
        1
    );
}

#[test]
fn invalid_and_conflicting_definitions_fail_before_use() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("CWR.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/runtime'\n[[exports]]\npath = './bad.toml'\nas = 'bad'\n").unwrap();
    std::fs::write(root.join("bad.toml"), format!("type = '{DEFINITION_TYPE}'\nref = '{TYPE}'\nstorage = 'toml'\n[value_schema]\ntype = 'object'\n")).unwrap();
    assert!(EntityCatalog::load(root.join("CWR.toml")).is_err());
    std::fs::write(root.join("bad.toml"), format!("type = '{DEFINITION_TYPE}'\nref = '{TYPE}'\nstorage = 'cwpdb'\n[value_schema]\ntype = 'object'\nrequired = 3\n")).unwrap();
    assert!(EntityCatalog::load(root.join("CWR.toml")).is_err());
}

#[test]
fn declared_formats_reject_invalid_refs_without_creating_entities() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("CWR.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/runtime'\n[[exports]]\npath = './typed.toml'\nas = 'typed'\n").unwrap();
    std::fs::write(root.join("typed.toml"), format!("type = '{DEFINITION_TYPE}'\nref = '{TYPE}'\nstorage = 'cwpdb'\n[value_schema]\ntype = 'object'\nrequired = ['owner']\n[value_schema.properties.owner]\ntype = 'string'\nformat = 'cwr-ref'\n")).unwrap();
    let catalog = EntityCatalog::load(root.join("CWR.toml")).unwrap();
    let mut store = NativeStore::open_in_memory().unwrap();
    assert!(catalog
        .create(&mut store, TYPE, REF, &json!({"owner":"not-a-ref"}))
        .is_err());
    assert!(store.get_entity(REF).is_err());
    catalog
        .create(
            &mut store,
            TYPE,
            REF,
            &json!({"owner":"cwp://com.example/owners/one"}),
        )
        .unwrap();
}
