#[test]
fn attached_image_crosses_host_and_codex_tool_boundaries_as_image_content() {
    let script = FAKE_CODEX
        .replace("cwr--send--message", "cwr--read--file")
        .replace(
            r#"{"to":"@helper","text":"hello"}"#,
            r#"{"file_ref":"cwp://example.test/files/image"}"#,
        )
        .replace(
            "    *'\"id\":3'*'\"result\"'*)",
            "    *'\"id\":3'*'\"imageUrl\":\"data:image/png;base64,'*'\"type\":\"inputImage\"'*)",
        );
    let (directory, executable) = fixture(&script);
    let mut requested = invocation(&executable);
    requested.host_operations = vec![dynamic_operation(
        "cwr.read_file",
        "Inspect an attached image.",
        json!({"type":"object","additionalProperties":false,"required":["file_ref"],"properties":{"file_ref":{"type":"string"}}}),
        json!({"type":"object"}),
    )];
    let mut calls = 0;
    let bytes = vec![42_u8; 5 * 1024 * 1024];
    let outcome = run_process_adapter_with_host_calls(&codex_adapter(directory.path()), requested,
        &ProcessAdapterStopControl::new(), |_| {}, |call| {
            calls += 1;
            assert_eq!(call.operation, "cwr.read_file");
            assert_eq!(call.input, json!({"file_ref":"cwp://example.test/files/image"}));
            Ok(json!({"metadata":{"file_ref":"cwp://example.test/files/image","owner_actor_ref":"cwp://example.test/actors/person",
                "filename":"screenshot.png","media_type":"image/png","size_bytes":bytes.len(),"sha256":"fixture"},"content_hex":hex::encode(&bytes)}))
        }).unwrap();
    assert_eq!(calls, 1);
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
}
