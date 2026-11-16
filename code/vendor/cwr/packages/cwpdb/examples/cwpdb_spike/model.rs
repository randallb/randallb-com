use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Result<T> = cwpdb::CwpdbResult<T>;
pub const ROOT: &str =
    "cwp://org.coworkerprotocol.local/runtimes/00000000-0000-4000-8000-000000000001";
pub const STORE_REF: &str = "cwp://org.coworkerprotocol.local/cwpdb-spike/store";
pub const SCHEMAS: &str = "cwp://org.coworkerprotocol/cwpdb/spike-2/schemas";
pub const EDGE_SCHEMA: &str = "cwp://org.coworkerprotocol/cwpdb/spike-2/schemas/edge";
pub const CONTAINS: &str = "contains";
pub const AUTHOR: &str = "authored_by";
pub const REPLY: &str = "replies_to";
pub const WORKSPACE: &str = "in_workspace";
pub const SHORTCUT: &str = "message_in_workspace";
pub const COLLECTION: &str = "in_collection";

pub fn schema(kind: &str) -> String {
    format!("{SCHEMAS}/{kind}")
}

pub fn reference(kind: &str, id: usize) -> String {
    format!("{ROOT}/{kind}/{id:032x}")
}

pub fn archive_ref(workspace: &str) -> String {
    format!("{workspace}/collections/archive")
}

pub fn ensure(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub reference: String,
    pub kind: String,
    pub schema_ref: String,
    pub revision: i64,
    pub props: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub reference: String,
    pub kind: String,
    pub schema_ref: String,
    pub source_ref: String,
    pub target_ref: String,
    pub position: i64,
    pub props: Value,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Out,
    In,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Order {
    Forward,
    Backward,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cursor {
    pub scope: String,
    pub kind: String,
    pub direction: Direction,
    pub position: i64,
    pub edge_ref: String,
}

pub struct Window<'a> {
    pub cursor: Option<&'a Cursor>,
    pub limit: usize,
    pub order: Order,
}

impl Window<'_> {
    pub fn first(limit: usize) -> Self {
        Self {
            cursor: None,
            limit,
            order: Order::Forward,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub has_more: bool,
    pub next_cursor: Option<Cursor>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Message {
    pub message_ref: String,
    pub author_actor_ref: String,
    pub reply_to_message_ref: Option<String>,
    pub text: String,
}

pub struct Append<'a> {
    pub message_ref: &'a str,
    pub conversation: &'a str,
    pub author: &'a str,
    pub text: &'a str,
    pub reply: Option<&'a str>,
}
