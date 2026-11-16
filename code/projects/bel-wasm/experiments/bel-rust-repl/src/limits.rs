//! Resource limits for the bootstrap host.
//!
//! Reading may admit more nodes than one host operation will traverse. That is
//! intentional: operations stop at their own work limit, while every admitted
//! value must still be safe to reject and discard.

pub(crate) const MAX_LINEAR_WORK: usize = 4_096;
pub(crate) const MAX_SOURCE_BYTES: usize = MAX_LINEAR_WORK * 16;
pub(crate) const MAX_READER_NODES: usize = MAX_LINEAR_WORK * 8;
pub(crate) const MAX_STRUCTURE_DEPTH: usize = 128;
pub(crate) const MAX_ABBREVIATION_PARTS: usize = MAX_LINEAR_WORK / 4;
pub(crate) const MAX_NUMERIC_COMPONENT: usize = MAX_LINEAR_WORK;

pub(crate) const MAX_OUTPUT_BYTES: usize = MAX_LINEAR_WORK * 4;
pub(crate) const MAX_RENDER_STEPS: usize = MAX_LINEAR_WORK * 2;
pub(crate) const MAX_LIST_TRAVERSAL_STEPS: usize = MAX_LINEAR_WORK * 2;

pub(crate) const MAX_EVAL_DEPTH: usize = MAX_STRUCTURE_DEPTH * 4;
pub(crate) const MAX_EVAL_STEPS: usize = MAX_LINEAR_WORK * MAX_LINEAR_WORK;
pub(crate) const MAX_CLEANUP_STEPS: usize = MAX_SOURCE_BYTES;
