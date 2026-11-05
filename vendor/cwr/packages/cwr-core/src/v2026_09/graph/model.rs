// Keep graph data models grouped by their role while sharing the graph module scope.
include!("model/constants.rs");
include!("model/checked.rs");
include!("model/resolved.rs");
include!("model/declarations.rs");
include!("model/loaded.rs");
