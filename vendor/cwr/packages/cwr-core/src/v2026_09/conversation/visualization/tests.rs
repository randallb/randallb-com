use super::*;

fn marker(path: &Path) -> String {
    format!(
        "{MARKER_START}{{\"path\":{}}}{MARKER_END}",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    )
}

#[test]
fn markers_are_standalone_and_outside_code() {
    let shown = marker(Path::new("/workspace/shown.html"));
    let fenced = marker(Path::new("/workspace/fenced.html"));
    let inline = marker(Path::new("/workspace/inline.html"));
    let indented = marker(Path::new("/workspace/indented.html"));
    let text =
        format!("Before\n{shown}\n```text\n{fenced}\n```\n`{inline}`\n    {indented}\nAfter");
    assert_eq!(
        visualization_markers(&text),
        vec![VisualizationMarker {
            path: "/workspace/shown.html".into(),
            title: None,
            mode: None,
        }]
    );
}

#[test]
fn markers_inside_multiline_code_spans_remain_literal() {
    let single = marker(Path::new("/workspace/single.html"));
    let double = marker(Path::new("/workspace/double.html"));
    let shown = marker(Path::new("/workspace/shown.html"));
    let text = format!("`before\n{single}\nafter`\n\n``before\n{double}\nafter``\n\n{shown}");
    assert_eq!(
        visualization_markers(&text),
        vec![VisualizationMarker {
            path: "/workspace/shown.html".into(),
            title: None,
            mode: None,
        }]
    );
}

#[test]
fn markers_inside_raw_html_blocks_remain_literal() {
    let hidden = marker(Path::new("/workspace/hidden.html"));
    let shown = marker(Path::new("/workspace/shown.html"));
    for block in [
        format!("<div>\n{hidden}\n</div>"),
        format!("- <div>\n  {hidden}\n  </div>"),
        format!("1. <div>\n   {hidden}\n   </div>"),
    ] {
        assert_eq!(
            visualization_markers(&format!("{block}\n\n{shown}")),
            vec![VisualizationMarker {
                path: "/workspace/shown.html".into(),
                title: None,
                mode: None,
            }],
            "{block}"
        );
    }
    for block in [
        format!("- <div>\n  {hidden}\n# Heading"),
        format!("1. <div>\n   {hidden}\n# Heading"),
        format!("- item\n  <div>\n  {hidden}\n# Heading"),
        format!("1. item\n   <div>\n   {hidden}\n# Heading"),
        format!("- <div>\n  {hidden}"),
        format!("1. <div>\n   {hidden}"),
    ] {
        assert_eq!(
            visualization_markers(&format!("{block}\n{shown}")),
            vec![VisualizationMarker {
                path: "/workspace/shown.html".into(),
                title: None,
                mode: None,
            }],
            "{block}"
        );
    }
    let tabbed = format!("- <div>\n\tinside\n  {hidden}\n\n{shown}");
    assert_eq!(
        visualization_markers(&tabbed),
        vec![VisualizationMarker {
            path: "/workspace/shown.html".into(),
            title: None,
            mode: None,
        }]
    );
    let tabbed_opener = format!("- item\n\t<div>\n  {hidden}\n\n{shown}");
    assert_eq!(
        visualization_markers(&tabbed_opener),
        vec![VisualizationMarker {
            path: "/workspace/shown.html".into(),
            title: None,
            mode: None,
        }]
    );
    let after_list = format!("- item\n\noutside\n  <div>\n{hidden}\n\n{shown}");
    assert_eq!(
        visualization_markers(&after_list),
        vec![VisualizationMarker {
            path: "/workspace/shown.html".into(),
            title: None,
            mode: None,
        }]
    );
    let nested = format!("- outer\n  - inner\n\n  outer\n  <div>\n  {hidden}\n# Heading\n{shown}");
    assert_eq!(
        visualization_markers(&nested),
        vec![VisualizationMarker {
            path: "/workspace/shown.html".into(),
            title: None,
            mode: None,
        }]
    );
}

#[test]
fn every_commonmark_html_block_type_hides_markers() {
    let hidden = marker(Path::new("/workspace/hidden.html"));
    let shown = marker(Path::new("/workspace/shown.html"));
    for block in [
        format!("<script>\n{hidden}\n</script>"),
        format!("<!--\n{hidden}\n-->"),
        format!("<?target\n{hidden}\n?>"),
        format!("<!DECLARATION\n{hidden}\n>"),
        format!("<![CDATA[\n{hidden}\n]]>"),
        format!("<custom-tag>\n{hidden}"),
    ] {
        assert_eq!(
            visualization_markers(&format!("{block}\n\n{shown}")),
            vec![VisualizationMarker {
                path: "/workspace/shown.html".into(),
                title: None,
                mode: None,
            }],
            "{block}"
        );
    }
}

#[test]
fn commonmark_list_padding_and_html_outdents_preserve_marker_boundaries() {
    let hidden = marker(Path::new("/workspace/hidden.html"));
    let shown = marker(Path::new("/workspace/shown.html"));
    let after = marker(Path::new("/workspace/after.html"));
    for text in [
        format!("-     code\n  <div>\n{hidden}\n\n{shown}"),
        format!("-\n\t<div>\n{hidden}\n\n{shown}"),
        format!("- - inner\n    <div>\n{hidden}\n\n{shown}"),
    ] {
        assert_eq!(
            visualization_markers(&text),
            vec![
                VisualizationMarker {
                    path: "/workspace/hidden.html".into(),
                    title: None,
                    mode: None,
                },
                VisualizationMarker {
                    path: "/workspace/shown.html".into(),
                    title: None,
                    mode: None,
                },
            ],
            "{text}"
        );
    }
    let outdent = format!("- item\n  <div>\n{shown}\n  <div>\n{hidden}\n\n{after}");
    assert_eq!(
        visualization_markers(&outdent),
        vec![
            VisualizationMarker {
                path: "/workspace/shown.html".into(),
                title: None,
                mode: None,
            },
            VisualizationMarker {
                path: "/workspace/after.html".into(),
                title: None,
                mode: None,
            },
        ]
    );
}

#[test]
fn ordered_lists_above_one_do_not_interrupt_multiline_code_spans() {
    let hidden = marker(Path::new("/workspace/hidden.html"));
    let text = format!("before `open\n2. ```\n{hidden}\nclose`");
    assert!(visualization_markers(&text).is_empty());
}

#[test]
fn list_looking_html_content_does_not_change_markdown_container_state() {
    let hidden = marker(Path::new("/workspace/hidden.html"));
    let shown = marker(Path::new("/workspace/shown.html"));
    let text = format!("<div>\n- fake\n\n  <div>\n{hidden}\n\n{shown}");
    assert_eq!(
        visualization_markers(&text),
        vec![VisualizationMarker {
            path: "/workspace/shown.html".into(),
            title: None,
            mode: None,
        }]
    );
}

#[test]
fn noninterrupting_ordered_list_text_does_not_change_container_state() {
    let hidden = marker(Path::new("/workspace/hidden.html"));
    let shown = marker(Path::new("/workspace/shown.html"));
    let text = format!("paragraph\n2. fake\n   <div>\n{hidden}\n\n{shown}");
    assert_eq!(
        visualization_markers(&text),
        vec![VisualizationMarker {
            path: "/workspace/shown.html".into(),
            title: None,
            mode: None,
        }]
    );
}

#[test]
fn nested_noninterrupting_ordered_text_does_not_change_container_state() {
    let hidden = marker(Path::new("/workspace/hidden.html"));
    let shown = marker(Path::new("/workspace/shown.html"));
    let text = format!("- paragraph\n  2. fake\n     <div>\n  {hidden}\n\n{shown}");
    assert_eq!(
        visualization_markers(&text),
        vec![VisualizationMarker {
            path: "/workspace/shown.html".into(),
            title: None,
            mode: None,
        }]
    );
}

#[test]
fn indented_inline_spans_continuing_blocks_hide_markers() {
    let hidden = marker(Path::new("/workspace/hidden.html"));
    for text in [
        format!("paragraph\n    `before\n{hidden}\nafter`"),
        format!("- item\n      `before\n{hidden}\nafter`"),
    ] {
        assert!(visualization_markers(&text).is_empty(), "{text}");
    }
}

#[test]
fn markers_inside_list_nested_fences_remain_literal() {
    let hidden = marker(Path::new("/workspace/hidden.html"));
    for text in [
        format!("- ```\n  {hidden}\n  ````"),
        format!("1. ~~~\n   {hidden}\n   ~~~"),
    ] {
        assert!(visualization_markers(&text).is_empty(), "{text}");
    }
}

#[test]
fn block_starters_end_inline_code_lookahead() {
    let shown = marker(Path::new("/workspace/shown.html"));
    for closing_block in ["# heading `", "> quote `", "- item `"] {
        let text = format!("`unclosed\n{shown}\n{closing_block}");
        assert_eq!(visualization_markers(&text).len(), 1, "{text}");
    }
}

#[test]
fn noninterrupting_lists_and_inline_html_stay_in_code_spans() {
    let hidden = marker(Path::new("/workspace/hidden.html"));
    for closing_line in [
        "2. closing `",
        "1. \nclosing `",
        "+ \nclosing `",
        "* \nclosing `",
        "<span>closing `</span>",
        "<![cdata[ closing `",
    ] {
        let text = format!("`before\n{hidden}\n{closing_line}");
        assert!(visualization_markers(&text).is_empty(), "{text}");
    }
}

#[test]
fn backticks_in_marker_json_do_not_hide_later_markers() {
    let first =
        format!("{MARKER_START}{{\"path\":\"/workspace/first.html\",\"title\":\"`\"}}{MARKER_END}");
    let second = marker(Path::new("/workspace/second.html"));
    assert_eq!(
        visualization_markers(&format!("{first}\n{second}")).len(),
        2
    );
}

#[test]
fn unmatched_inline_delimiter_cannot_expose_a_fenced_marker() {
    let hidden = marker(Path::new("/workspace/inside.html"));
    let text = format!("`unclosed\n```text\n`\n{hidden}\n```\n");
    assert!(visualization_markers(&text).is_empty());
}

#[test]
fn fenced_backticks_cannot_hide_a_marker_before_the_fence() {
    let shown = marker(Path::new("/workspace/outside.html"));
    let text = format!("`unclosed\n{shown}\n```text\n`\n```\n");
    assert_eq!(
        visualization_markers(&text),
        vec![VisualizationMarker {
            path: "/workspace/outside.html".into(),
            title: None,
            mode: None,
        }]
    );
}

#[test]
fn unmatched_and_escaped_backticks_do_not_hide_later_markers() {
    let after_unmatched = marker(Path::new("/workspace/after-unmatched.html"));
    let after_escaped = marker(Path::new("/workspace/after-escaped.html"));
    let text = format!("Unmatched `\n\n{after_unmatched}\n\nEscaped \\`\n{after_escaped}\n");
    assert_eq!(
        visualization_markers(&text),
        vec![
            VisualizationMarker {
                path: "/workspace/after-unmatched.html".into(),
                title: None,
                mode: None,
            },
            VisualizationMarker {
                path: "/workspace/after-escaped.html".into(),
                title: None,
                mode: None,
            },
        ]
    );
}

#[test]
fn many_unmatched_backtick_runs_do_not_require_repeated_lookahead() {
    let runs = (1..=500)
        .map(|length| "`".repeat(length))
        .collect::<Vec<_>>()
        .join("x");
    let shown = marker(Path::new("/workspace/shown.html"));
    assert_eq!(
        visualization_markers(&format!("{runs}\n\n{shown}")).len(),
        1
    );
}

#[test]
fn malformed_and_incomplete_markers_remain_unrecognized() {
    let text = format!(
        "{MARKER_START}{{not json}}{MARKER_END}\n{MARKER_START}{{\"path\":\"/workspace/incomplete.html\"}}"
    );
    assert!(visualization_markers(&text).is_empty());
}

#[test]
fn duplicate_marker_fields_match_json_parse_last_value_behavior() {
    let text = format!(
        "{MARKER_START}{{\"path\":\"/workspace/first.html\",\"path\":\"/workspace/last.html\",\"title\":\"First\",\"title\":\"Last\"}}{MARKER_END}"
    );
    assert_eq!(
        visualization_markers(&text),
        vec![VisualizationMarker {
            path: "/workspace/last.html".into(),
            title: Some("Last".into()),
            mode: None,
        }]
    );
}

#[test]
fn marker_parsing_rejects_unknown_fields_on_both_runtimes() {
    let text = format!(
        "{MARKER_START}{{\"path\":\"/workspace/chart.html\",\"ignored\":1e400}}{MARKER_END}"
    );
    assert!(visualization_markers(&text).is_empty());
}

#[test]
fn external_projection_replaces_markers_and_drops_artifact_references() {
    let text = format!(
        "Summary\n\n{MARKER_START}{{\"path\":\"/Users/example/private/chart.html\",\"title\":\"Saved chart\"}}{MARKER_END}"
    );
    let message: ConversationMessage = serde_json::from_value(serde_json::json!({
        "message_ref":"cwp://example.test/messages/visualization",
        "author_actor_ref":"cwp://example.test/actors/helper",
        "addressed_actor_refs":[],
        "text":text,
        "visualizations":[{
            "marker_index":0,
            "title":"Saved chart",
            "artifact_ref":"cwp://example.test/visualizations/private"
        }]
    }))
    .unwrap();

    let projected = message.clone_for_external_delivery();
    assert_eq!(projected.text, "Summary\n\nSaved chart");
    assert!(projected.visualizations.is_empty());
    let serialized = serde_json::to_string(&projected).unwrap();
    assert!(!serialized.contains("/Users/example/private"));
    assert!(!serialized.contains("visualizations/private"));
}

#[test]
fn collection_snapshots_a_regular_workspace_html_file() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("chart.html");
    fs::write(&path, "<button>Interactive</button>").unwrap();
    assert_eq!(
        read_visualization(temporary.path(), path.to_str().unwrap()).unwrap(),
        "<button>Interactive</button>"
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn collection_rejects_escape_symlink_and_oversized_files() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.html");
    fs::write(&secret, "secret").unwrap();
    assert_eq!(
        read_visualization(temporary.path(), secret.to_str().unwrap()),
        Err("The visualization is outside the coworker workspace.")
    );

    let link = temporary.path().join("linked.html");
    symlink(&secret, &link).unwrap();
    assert_eq!(
        read_visualization(temporary.path(), link.to_str().unwrap()),
        Err("The visualization file is unavailable or is not a regular file.")
    );

    let oversized = temporary.path().join("oversized.html");
    fs::write(&oversized, vec![b'x'; MAX_VISUALIZATION_BYTES + 1]).unwrap();
    assert_eq!(
        read_visualization(temporary.path(), oversized.to_str().unwrap()),
        Err("The visualization exceeds the 1 MB limit.")
    );
}
