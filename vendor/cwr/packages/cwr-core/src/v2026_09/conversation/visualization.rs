use super::*;
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use std::{
    collections::BTreeSet,
    ffi::CString,
    fs::File,
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::{Component, Path},
};

const MARKER_START: &str = "\u{e200}visualize\u{e202}";
const MARKER_END: &str = "\u{e201}";
pub(super) const MAX_VISUALIZATION_BYTES: usize = 1024 * 1024;
const MAX_VISUALIZATIONS_PER_MESSAGE: usize = 4;

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct VisualizationMarker {
    path: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

struct LocatedVisualizationMarker {
    start: usize,
    end: usize,
    marker: VisualizationMarker,
}

pub(super) fn collect_for_invocation(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    text: &str,
) -> Vec<ConversationVisualization> {
    let markers = visualization_markers(text);
    if markers.is_empty() {
        return Vec::new();
    }
    let working_path = invocation::execution_conversation_in_state(state, invocation_ref)
        .and_then(working::snapshot)
        .ok()
        .and_then(|invocation| invocation.retained_route)
        // The marker is a Codex local-file contract. Remote, person, and
        // intrinsic routes can retain a local host working directory too, but
        // their response did not originate in that directory and must never
        // turn message text into a host file read.
        .filter(|route| route.adapter_ref() == crate::adapter::CODEX_ADAPTER_REF)
        .map(|route| route.working_path().to_owned());

    markers
        .into_iter()
        .take(MAX_VISUALIZATIONS_PER_MESSAGE)
        .enumerate()
        .map(|(marker_index, marker)| {
            let result = working_path
                .as_deref()
                .ok_or("This visualization was not produced by a local workspace execution.")
                .and_then(|root| read_visualization(root, &marker.path));
            match result {
                Ok(html) => {
                    let artifact_ref = new_ref(&state.runtime_root_ref, "visualization-artifacts");
                    state.visualization_artifacts.insert(
                        artifact_ref.clone(),
                        ConversationVisualizationArtifact {
                            artifact_ref: artifact_ref.clone(),
                            html,
                        },
                    );
                    ConversationVisualization {
                        marker_index: marker_index as u32,
                        title: normalized_title(marker.title),
                        mode: normalized_mode(marker.mode),
                        artifact_ref: Some(artifact_ref),
                        error: None,
                    }
                }
                Err(error) => ConversationVisualization {
                    marker_index: marker_index as u32,
                    title: normalized_title(marker.title),
                    mode: normalized_mode(marker.mode),
                    artifact_ref: None,
                    error: Some(error.into()),
                },
            }
        })
        .collect()
}

pub(super) fn validate_persisted(message: &ConversationMessage) -> CwrResult<()> {
    if message.visualizations.len() > MAX_VISUALIZATIONS_PER_MESSAGE {
        return invalid("Conversation messages may contain at most four visualizations");
    }
    let marker_count = visualization_markers(&message.text).len();
    for (index, visualization) in message.visualizations.iter().enumerate() {
        if visualization.marker_index as usize != index || index >= marker_count {
            return invalid("Conversation visualization marker indexes are invalid");
        }
        if visualization
            .title
            .as_ref()
            .is_some_and(|title| title.chars().count() > 200)
        {
            return invalid("Conversation visualization titles may contain at most 200 characters");
        }
        if visualization
            .mode
            .as_deref()
            .is_some_and(|mode| mode != "wide")
        {
            return invalid("Conversation visualization mode is unsupported");
        }
        match (&visualization.artifact_ref, &visualization.error) {
            (Some(artifact_ref), None) if artifact_ref.starts_with("cwp://") => {}
            (None, Some(error)) if !error.is_empty() && error.len() <= 500 => {}
            _ => return invalid("Conversation visualization content is invalid"),
        }
    }
    Ok(())
}

pub(super) fn validate_artifacts(state: &ConversationRuntimeState) -> CwrResult<()> {
    for artifact in state.visualization_artifacts.values() {
        validate_artifact(artifact)?;
    }
    for conversation in &state.conversations {
        for message in &conversation.messages {
            validate_persisted(message)?;
        }
    }
    Ok(())
}

pub(super) fn validate_artifact(artifact: &ConversationVisualizationArtifact) -> CwrResult<()> {
    if artifact.html.len() > MAX_VISUALIZATION_BYTES {
        return invalid("Conversation visualization artifact exceeds the 1 MB limit");
    }
    Ok(())
}

impl ConversationAdmissionStore {
    pub fn visualization_artifact(
        &self,
        artifact_ref: &str,
    ) -> CwrResult<ConversationVisualizationArtifact> {
        let record = self
            .backend
            .lock()
            .map_err(|_| invalid_error("conversation backend lock poisoned"))?
            .get(artifact_ref)?
            .filter(|record| {
                record.kind == cwpdb_store::conversation::RecordKind::VisualizationArtifact
            })
            .ok_or_else(|| invalid_error("unknown conversation visualization artifact"))?;
        let artifact: ConversationVisualizationArtifact =
            serde_json::from_value(record.properties["record"].clone())?;
        validate_artifact(&artifact)?;
        if artifact.artifact_ref != artifact_ref {
            return invalid("Conversation visualization artifact reference is invalid");
        }
        Ok(artifact)
    }
}

fn normalized_title(title: Option<String>) -> Option<String> {
    title
        .map(|title| title.trim().chars().take(200).collect::<String>())
        .filter(|title| !title.is_empty())
}

fn normalized_mode(mode: Option<String>) -> Option<String> {
    mode.filter(|mode| mode == "wide")
}

fn visualization_markers(text: &str) -> Vec<VisualizationMarker> {
    located_visualization_markers(text)
        .into_iter()
        .map(|located| located.marker)
        .collect()
}

pub(super) fn preview_text(message: &ConversationMessage) -> String {
    let markers = located_visualization_markers(&message.text);
    if markers.is_empty() {
        return message.text.clone();
    }
    let mut preview = String::new();
    let mut cursor = 0;
    for (index, located) in markers.into_iter().enumerate() {
        preview.push_str(&message.text[cursor..located.start]);
        let title = message
            .visualizations
            .iter()
            .find(|visualization| visualization.marker_index == index as u32)
            .and_then(|visualization| visualization.title.as_deref())
            .or(located.marker.title.as_deref())
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or("Visualization");
        preview.push_str(title);
        cursor = located.end;
    }
    preview.push_str(&message.text[cursor..]);
    preview
}

impl ConversationMessage {
    pub fn nonvisual_text(&self) -> String {
        preview_text(self)
    }

    pub fn clone_without_visualizations(&self) -> Self {
        let mut message = self.clone();
        message.visualizations.clear();
        message
    }

    pub fn clone_for_external_delivery(&self) -> Self {
        let mut message = self.clone_without_visualizations();
        message.text = preview_text(self);
        message
    }
}

fn located_visualization_markers(text: &str) -> Vec<LocatedVisualizationMarker> {
    let mut markers: Vec<LocatedVisualizationMarker> = Vec::new();
    let line_starts = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(offset, _)| offset + 1))
        .collect::<Vec<_>>();
    let mut eligible_lines = BTreeSet::new();
    let mut inside_code_block = false;
    for (event, range) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                inside_code_block = true;
                continue;
            }
            Event::End(TagEnd::CodeBlock) => {
                inside_code_block = false;
                continue;
            }
            Event::Text(_) if !inside_code_block => {}
            _ => continue,
        }
        let mut line_index = line_starts
            .partition_point(|line_start| *line_start <= range.start)
            .saturating_sub(1);
        while line_starts
            .get(line_index)
            .is_some_and(|line_start| *line_start < range.end)
        {
            eligible_lines.insert(line_index);
            line_index += 1;
        }
    }
    for line_index in eligible_lines {
        let line_start = line_starts[line_index];
        let line_end = line_starts
            .get(line_index + 1)
            .map_or(text.len(), |next_start| next_start - 1);
        let source = text[line_start..line_end].trim_end_matches('\r');
        if source.starts_with('\t') || source.bytes().take_while(|byte| *byte == b' ').count() > 3 {
            continue;
        }
        if let Some(marker) = parse_visualization_marker(source.trim()) {
            markers.push(LocatedVisualizationMarker {
                start: line_start,
                end: line_end,
                marker,
            });
        }
    }
    markers
}

fn parse_visualization_marker(candidate: &str) -> Option<VisualizationMarker> {
    let payload = candidate
        .strip_prefix(MARKER_START)?
        .strip_suffix(MARKER_END)?;
    if payload.contains(MARKER_START) || payload.contains(MARKER_END) {
        return None;
    }
    serde_json::from_str::<Value>(payload)
        .ok()
        .and_then(|value| serde_json::from_value::<VisualizationMarker>(value).ok())
        .filter(|marker| !marker.path.is_empty())
}

fn read_visualization(root: &Path, marker_path: &str) -> Result<String, &'static str> {
    let path = Path::new(marker_path);
    if !path.is_absolute() {
        return Err("Visualization paths must be absolute.");
    }
    if !matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some(extension) if extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
    ) {
        return Err("Visualization files must use an .html extension.");
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "The visualization workspace is unavailable.")?;
    if canonical_root.parent().is_none() {
        return Err("Visualization collection requires a scoped workspace.");
    }
    let relative = path
        .strip_prefix(root)
        .or_else(|_| path.strip_prefix(&canonical_root))
        .map_err(|_| "The visualization is outside the coworker workspace.")?;
    let components = relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => Ok(value),
            _ => Err("Visualization paths cannot contain symbolic traversal."),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (name, parents) = components
        .split_last()
        .ok_or("The visualization path does not identify a file.")?;
    let mut directory = open_absolute_directory(&canonical_root)
        .map_err(|_| "The visualization workspace is unavailable.")?;
    for component in parents {
        directory = open_directory_at(&directory, component)
            .map_err(|_| "Visualization paths cannot use symbolic links.")?;
    }
    let mut file = open_regular_file_at(&directory, name)
        .map_err(|_| "The visualization file is unavailable or is not a regular file.")?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_VISUALIZATION_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "The visualization file could not be read.")?;
    if bytes.len() > MAX_VISUALIZATION_BYTES {
        return Err("The visualization exceeds the 1 MB limit.");
    }
    String::from_utf8(bytes).map_err(|_| "The visualization must be UTF-8 HTML.")
}

fn open_absolute_directory(path: &Path) -> io::Result<File> {
    let mut directory = File::open("/")?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(component) => {
                directory = open_directory_at(&directory, component)?;
            }
            _ => return Err(io::Error::other("path is not canonical")),
        }
    }
    Ok(directory)
}

fn open_directory_at(directory: &File, name: &std::ffi::OsStr) -> io::Result<File> {
    let name = CString::new(name.as_bytes())?;
    // SAFETY: openat borrows a live directory descriptor and C string. The
    // successful descriptor is transferred exactly once into File.
    let raw = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: raw is newly owned after the successful openat call.
    Ok(unsafe { File::from_raw_fd(raw) })
}

fn open_regular_file_at(directory: &File, name: &std::ffi::OsStr) -> io::Result<File> {
    let name = CString::new(name.as_bytes())?;
    // SAFETY: openat borrows a live directory descriptor and C string. The
    // successful descriptor is transferred exactly once into File.
    let raw = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: raw is newly owned after the successful openat call.
    let file = unsafe { File::from_raw_fd(raw) };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("visualization is not a regular file"));
    }
    Ok(file)
}

#[cfg(test)]
#[path = "visualization/tests.rs"]
mod tests;
