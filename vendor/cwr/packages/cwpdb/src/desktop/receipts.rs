//! Native-owner request replay. Payloads live only until the next accepted
//! mutation of this owner; expired markers retain no arguments or result data.
use super::{OperationError, OperationErrorCode as Code};
use crate::{
    self as cwpdb,
    declared::{Catalog as Declarations, EntityBinding as EntityCatalog},
    ref_read::RefReadError,
    unordered::{self, Cardinality, Contract},
};
use rusqlite::{Connection, Transaction};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
const DEFINITION: &str = "cwp://org.coworkerprotocol/cwr/entities/desktop-registry-receipt";
const KIND: &str = "desktop_registry_receipt_v1";
const SCHEMA: &str =
    "cwp://org.coworkerprotocol/cwpdb/schemas/desktop_registry_receipt_v1/2026-10-23";
type Result<T> = std::result::Result<T, OperationError>;
fn failure(code: Code) -> OperationError {
    OperationError { code }
}
fn storage(_: impl std::fmt::Display) -> OperationError {
    failure(Code::StorageFailure)
}
fn relation(error: unordered::Error) -> OperationError {
    failure(match error {
        unordered::Error::Storage(_) => Code::StorageFailure,
        unordered::Error::RevisionConflict => Code::RevisionConflict,
        unordered::Error::RelationshipsExist => Code::RelationshipsExist,
        _ => Code::UnsupportedDomainContract,
    })
}
pub enum Replay<T> {
    Absent,
    Retained(T),
    Expired,
}
pub struct Receipts {
    catalog: EntityCatalog,
    contract: Contract,
}
impl Receipts {
    pub fn new(declarations: &Declarations, source_kind: &'static str) -> Result<Self> {
        Ok(Self {
            catalog: declarations
                .entity("cwp://org.coworkerprotocol/cwr/entities/desktop-registry-receipt")
                .map_err(storage)?,
            contract: Contract {
                kind: KIND,
                schema: SCHEMA,
                source_kind,
                target_kind: KIND,
                external_target: false,
                source_cardinality: Cardinality::One,
                target_cardinality: Cardinality::One,
            },
        })
    }
    pub fn replay<T: DeserializeOwned>(
        &self,
        db: &Connection,
        request_id: &str,
        arguments: &Value,
    ) -> Result<Replay<T>> {
        uuid::Uuid::parse_str(request_id).map_err(|_| failure(Code::InvalidInput))?;
        let reference = reference(request_id);
        match cwpdb::ref_read::get_ref(db, &reference) {
            Err(RefReadError::NotFound) => return Ok(Replay::Absent),
            Err(_) => return Err(failure(Code::StorageFailure)),
            Ok(_) => {}
        }
        let record = self
            .catalog
            .get_in_connection(db, DEFINITION, &reference)
            .map_err(|_| failure(Code::UnsupportedDomainContract))?;
        let payload = &record.value.properties["record"];
        if payload["expired"] == true {
            return Ok(Replay::Expired);
        }
        if payload["arguments"] != *arguments {
            return Err(failure(Code::RequestConflict));
        }
        let response = serde_json::from_value(payload["result"].clone())
            .map_err(|_| failure(Code::UnsupportedDomainContract))?;
        Ok(Replay::Retained(response))
    }
    pub fn expire(&self, tx: &Transaction<'_>, owner: &str) -> Result<()> {
        if let Some(edge) =
            unordered::single_outgoing(tx, self.contract, owner).map_err(relation)?
        {
            let record = self
                .catalog
                .get_in_connection(tx, DEFINITION, &edge.target)
                .map_err(|_| failure(Code::UnsupportedDomainContract))?;
            self.catalog
                .replace_in_transaction(
                    tx,
                    DEFINITION,
                    &edge.target,
                    &record.revision,
                    &json!({"record":{"expired":true}}),
                )
                .map_err(storage)?;
            unordered::unlink(tx, self.contract, owner, &edge.target, edge.revision)
                .map_err(relation)?;
        }
        Ok(())
    }
    pub fn save<T: Serialize>(
        &self,
        tx: &Transaction<'_>,
        request_id: &str,
        arguments: &Value,
        response: &T,
        owner: Option<&str>,
    ) -> Result<()> {
        let value = if owner.is_some() {
            json!({"record":{"expired":false,"arguments":arguments,"result":response}})
        } else {
            json!({"record":{"expired":true}})
        };
        let reference = reference(request_id);
        self.catalog
            .create_in_transaction(tx, DEFINITION, &reference, &value)
            .map_err(storage)?;
        if let Some(owner) = owner {
            unordered::link(tx, self.contract, owner, &reference).map_err(relation)?;
        }
        Ok(())
    }
}
fn reference(id: &str) -> String {
    unordered::desktop_ref("registry-request", &json!(id))
}
