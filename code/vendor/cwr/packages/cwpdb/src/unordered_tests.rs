use super::*;
const CONTRACT: Contract = Contract {
    kind: "test_membership",
    schema: "cwp://fixture/membership-schema",
    source_kind: "owner",
    target_kind: "target",
    external_target: false,
    source_cardinality: Cardinality::One,
    target_cardinality: Cardinality::One,
};
#[test]
fn endpoints_cardinality_stale_unlink_and_non_cascading_delete() -> CwpdbResult<()> {
    let mut store = crate::NativeStore::open_in_memory()?;
    store.graph_transaction(|tx| {
        crate::put_node(tx, "cwp://fixture/owner", "owner", &json!({}))?;
        crate::put_node(tx, "cwp://fixture/target", "target", &json!({}))?;
        crate::put_node(tx, "cwp://fixture/other", "target", &json!({}))?;
        link(tx, CONTRACT, "cwp://fixture/owner", "cwp://fixture/target")?;
        assert!(matches!(
            link(tx, CONTRACT, "cwp://fixture/owner", "cwp://fixture/other"),
            Err(Error::Cardinality)
        ));
        assert!(matches!(
            link(tx, CONTRACT, "cwp://fixture/owner", "cwp://fixture/owner"),
            Err(Error::Endpoint)
        ));
        assert!(matches!(
            delete_unlinked_entity(tx, "cwp://fixture/owner"),
            Err(Error::RelationshipsExist)
        ));
        let first = single_outgoing(tx, CONTRACT, "cwp://fixture/owner")?.unwrap();
        unlink(
            tx,
            CONTRACT,
            "cwp://fixture/owner",
            &first.target,
            first.revision,
        )?;
        link(tx, CONTRACT, "cwp://fixture/owner", "cwp://fixture/target")?;
        let second = single_outgoing(tx, CONTRACT, "cwp://fixture/owner")?.unwrap();
        assert!(second.revision > first.revision);
        assert!(matches!(
            unlink(
                tx,
                CONTRACT,
                "cwp://fixture/owner",
                &first.target,
                first.revision
            ),
            Err(Error::RevisionConflict)
        ));
        assert!(crate::record_exists(tx, "cwp://fixture/target")?);
        Ok(())
    })?;
    Ok(())
}
