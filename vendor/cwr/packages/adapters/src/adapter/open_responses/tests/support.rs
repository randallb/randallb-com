fn response(output: Vec<Value>) -> Value {
    json!({
        "id":"resp_test",
        "object":"response",
        "status":"completed",
        "output":output,
        "error":null
    })
}

fn message(id: &str, text: &str) -> Value {
    json!({
        "type":"message","id":id,"role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":text,"annotations":[]}]
    })
}

fn function_call(id: &str, call_id: &str, name: &str, arguments: Value) -> Value {
    json!({
        "type":"function_call","id":id,"call_id":call_id,"name":name,
        "arguments":serde_json::to_string(&arguments).unwrap(),"status":"completed"
    })
}

struct MockReply {
    status: u16,
    body: Vec<u8>,
}

impl MockReply {
    fn ok(body: Value) -> Self {
        Self::json(200, body)
    }

    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn raw(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.as_bytes().to_vec(),
        }
    }
}

struct CapturedRequest {
    headers: String,
    body: Value,
}

struct MockServer {
    endpoint: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    thread: thread::JoinHandle<()>,
}

struct GatedStreamServer {
    endpoint: String,
    commentary_delta_sent: mpsc::Receiver<()>,
    release_commentary_delta: mpsc::SyncSender<()>,
    assistant_delta_sent: mpsc::Receiver<()>,
    release_assistant_delta: mpsc::SyncSender<()>,
    thread: thread::JoinHandle<CapturedRequest>,
}

impl GatedStreamServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let (commentary_sender, commentary_delta_sent) = mpsc::sync_channel(1);
        let (release_commentary_delta, commentary_release) = mpsc::sync_channel(1);
        let (assistant_sender, assistant_delta_sent) = mpsc::sync_channel(1);
        let (release_assistant_delta, assistant_release) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let first = [
                stream_event(json!({
                    "type":"response.created","sequence_number":0,
                    "response":{"id":"resp_stream","status":"in_progress","output":[]}
                })),
                stream_event(json!({
                    "type":"response.output_item.added","sequence_number":1,"output_index":0,
                    "item":{"type":"reasoning","id":"reason_stream","status":"in_progress",
                        "summary":[]}
                })),
                stream_event(json!({
                    "type":"response.reasoning_summary_text.delta","sequence_number":2,
                    "item_id":"reason_stream","output_index":0,"summary_index":0,
                    "delta":"Checking"
                })),
            ]
            .concat();
            stream.write_all(first.as_bytes()).unwrap();
            stream.flush().unwrap();
            commentary_sender.send(()).unwrap();
            commentary_release.recv().unwrap();

            let reasoning = json!({
                "type":"reasoning","id":"reason_stream","status":"completed",
                "summary":[{"type":"summary_text","text":"Checking the request."}]
            });
            let assistant_start = [
                stream_event(json!({
                    "type":"response.reasoning_summary_text.done","sequence_number":3,
                    "item_id":"reason_stream","output_index":0,"summary_index":0,
                    "text":"Checking the request."
                })),
                stream_event(json!({
                    "type":"response.output_item.done","sequence_number":4,"output_index":0,
                    "item":reasoning.clone()
                })),
                stream_event(json!({
                    "type":"response.output_item.added","sequence_number":5,"output_index":1,
                    "item":{"type":"message","id":"msg_stream","status":"in_progress",
                        "role":"assistant","content":[]}
                })),
                stream_event(json!({
                    "type":"response.content_part.added","sequence_number":6,
                    "item_id":"msg_stream","output_index":1,"content_index":0,
                    "part":{"type":"output_text","text":"","annotations":[]}
                })),
                stream_event(json!({
                    "type":"response.output_text.delta","sequence_number":7,
                    "item_id":"msg_stream","output_index":1,"content_index":0,"delta":"Hello"
                })),
            ]
            .concat();
            stream.write_all(assistant_start.as_bytes()).unwrap();
            stream.flush().unwrap();
            assistant_sender.send(()).unwrap();
            assistant_release.recv().unwrap();

            let final_message = message("msg_stream", "Hello world.");
            let completed = json!({
                "id":"resp_stream","object":"response","status":"completed",
                "output":[reasoning, final_message.clone()],"error":null
            });
            let rest = [
                stream_event(json!({
                    "type":"response.output_text.delta","sequence_number":8,
                    "item_id":"msg_stream","output_index":1,"content_index":0,"delta":" world."
                })),
                stream_event(json!({
                    "type":"response.output_text.done","sequence_number":9,
                    "item_id":"msg_stream","output_index":1,"content_index":0,
                    "text":"Hello world."
                })),
                stream_event(json!({
                    "type":"response.content_part.done","sequence_number":10,
                    "item_id":"msg_stream","output_index":1,"content_index":0,
                    "part":{"type":"output_text","text":"Hello world.","annotations":[]}
                })),
                stream_event(json!({
                    "type":"response.output_item.done","sequence_number":11,"output_index":1,
                    "item":final_message
                })),
                stream_event(json!({
                    "type":"response.completed","sequence_number":12,"response":completed
                })),
            ]
            .concat();
            stream.write_all(rest.as_bytes()).unwrap();
            stream.flush().unwrap();
            request
        });
        Self {
            endpoint,
            commentary_delta_sent,
            release_commentary_delta,
            assistant_delta_sent,
            release_assistant_delta,
            thread,
        }
    }

    fn wait_for_commentary_delta(&self) {
        self.commentary_delta_sent
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
    }

    fn release_commentary(&self) {
        self.release_commentary_delta.send(()).unwrap();
    }

    fn wait_for_assistant_delta(&self) {
        self.assistant_delta_sent
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
    }

    fn release_assistant(&self) {
        self.release_assistant_delta.send(()).unwrap();
    }

    fn finish(self) -> CapturedRequest {
        self.thread.join().unwrap()
    }
}

fn stream_event(event: Value) -> String {
    format!(
        "event: {}\ndata: {}\n\n",
        event["type"].as_str().unwrap(),
        serde_json::to_string(&event).unwrap()
    )
}

impl MockServer {
    fn start(replies: Vec<MockReply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let thread = thread::spawn(move || {
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                captured.lock().unwrap().push(request);
                let body = reply.body;
                let reason = if reply.status == 200 {
                    "OK"
                } else {
                    "Unauthorized"
                };
                write!(
                    stream,
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.status,
                    reason,
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        Self {
            endpoint,
            requests,
            thread,
        }
    }

    fn finish(self) -> Vec<CapturedRequest> {
        self.thread.join().unwrap();
        Arc::into_inner(self.requests)
            .unwrap()
            .into_inner()
            .unwrap()
    }
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::trim)
                .map(str::parse::<usize>)
        })
        .unwrap()
        .unwrap();
    while bytes.len() - header_end < content_length {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&buffer[..read]);
    }
    let body = serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap();
    CapturedRequest { headers, body }
}
