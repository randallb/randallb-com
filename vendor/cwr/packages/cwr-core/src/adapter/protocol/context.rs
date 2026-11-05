// Context has its own stream; control and exact typed action values remain
// ordinary capped frames. Chunk boundaries have no conversation semantics.
const CONTEXT_CHUNK_BYTES: usize = 64 * 1024;

fn context_stream_descriptor() -> Value {
    json!({"context_stream": {"format": "json", "version": 1}})
}

fn prepare_adapter_invoke(
    mut invocation: AdapterInvocation,
) -> CwrResult<(Vec<u8>, Option<Value>)> {
    // Validate every control byte before starting the adapter. Move the source
    // Value into the stream without cloning or serializing the whole history.
    let context = if invocation
        .thread
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        None
    } else {
        // The invocation ref is repeated in chunk headers; validate the largest
        // such frame too, rather than failing halfway through initialization.
        encode_adapter_message(&ContextFrame::ContextChunk {
            invocation_ref: invocation.invocation_ref.clone(),
            sequence: u64::MAX,
            data: "f".repeat(CONTEXT_CHUNK_BYTES * 2),
        })?;
        Some(std::mem::replace(
            &mut invocation.thread,
            context_stream_descriptor(),
        ))
    };
    let encoded = encode_adapter_message(&AdapterHostMessage::Invoke {
        protocol: PROCESS_ADAPTER_PROTOCOL_REF.into(),
        invocation: Box::new(invocation),
    })?;
    Ok((encoded, context))
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ContextFrame {
    ContextChunk {
        invocation_ref: String,
        sequence: u64,
        data: String,
    },
    ContextEnd {
        invocation_ref: String,
        chunks: u64,
    },
}

struct ContextStreamWriter<'a, W> {
    writer: &'a mut W,
    invocation_ref: &'a str,
    pending: Vec<u8>,
    chunks: u64,
    source_bytes: u64,
    max_frame_bytes: usize,
}

impl<'a, W: Write> ContextStreamWriter<'a, W> {
    fn new(writer: &'a mut W, invocation_ref: &'a str) -> Self {
        Self {
            writer,
            invocation_ref,
            pending: Vec::with_capacity(CONTEXT_CHUNK_BYTES),
            chunks: 0,
            source_bytes: 0,
            max_frame_bytes: 0,
        }
    }

    fn emit_chunk(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let frame = ContextFrame::ContextChunk {
            invocation_ref: self.invocation_ref.into(),
            sequence: self.chunks + 1,
            // Hex allows arbitrary byte boundaries, including the middle of a
            // UTF-8 character or JSON escape, without splitting source records.
            data: hex::encode(&self.pending),
        };
        let encoded = encode_adapter_message(&frame).map_err(io::Error::other)?;
        self.writer.write_all(&encoded)?;
        self.writer.flush()?;
        self.max_frame_bytes = self.max_frame_bytes.max(encoded.len());
        self.source_bytes += self.pending.len() as u64;
        self.pending.clear();
        self.chunks += 1;
        Ok(())
    }

    fn finish(mut self) -> CwrResult<AdapterContextDeliveryMetrics> {
        self.emit_chunk()?;
        write_adapter_message(
            self.writer,
            &ContextFrame::ContextEnd {
                invocation_ref: self.invocation_ref.into(),
                chunks: self.chunks,
            },
        )?;
        Ok(AdapterContextDeliveryMetrics {
            source_bytes: self.source_bytes,
            chunks: self.chunks,
            max_chunk_frame_bytes: self.max_frame_bytes,
            invoke_frame_bytes: 0,
            protocol_queue_limit_bytes: PROTOCOL_QUEUE_BYTES,
        })
    }
}

impl<W: Write> Write for ContextStreamWriter<'_, W> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let count = bytes.len();
        while !bytes.is_empty() {
            let take = bytes.len().min(CONTEXT_CHUNK_BYTES - self.pending.len());
            self.pending.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.pending.len() == CONTEXT_CHUNK_BYTES {
                self.emit_chunk()?;
            }
        }
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.emit_chunk()?;
        self.writer.flush()
    }
}

fn write_context_stream(
    writer: &mut impl Write,
    invocation_ref: &str,
    context: &Value,
) -> CwrResult<AdapterContextDeliveryMetrics> {
    let mut stream = ContextStreamWriter::new(writer, invocation_ref);
    serde_json::to_writer(&mut stream, context)?;
    stream.finish()
}

struct ContextStreamReader<'a, R> {
    reader: &'a mut R,
    invocation_ref: String,
    pending: io::Cursor<Vec<u8>>,
    chunks: u64,
    complete: bool,
}

impl<'a, R: BufRead> ContextStreamReader<'a, R> {
    fn new(reader: &'a mut R, invocation_ref: String) -> Self {
        Self {
            reader,
            invocation_ref,
            pending: io::Cursor::new(Vec::new()),
            chunks: 0,
            complete: false,
        }
    }

    fn next_chunk(&mut self) -> io::Result<()> {
        let line = read_protocol_line(self.reader)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "adapter context stream ended early",
            )
        })?;
        let frame: ContextFrame = serde_json::from_str(&line)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        match frame {
            ContextFrame::ContextChunk {
                invocation_ref,
                sequence,
                data,
            } if invocation_ref == self.invocation_ref && sequence == self.chunks + 1 => {
                if data.is_empty() || data.len() > CONTEXT_CHUNK_BYTES * 2 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "adapter context chunk has an invalid size",
                    ));
                }
                let bytes = hex::decode(data)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                self.pending = io::Cursor::new(bytes);
                self.chunks += 1;
            }
            ContextFrame::ContextEnd {
                invocation_ref,
                chunks,
            } if invocation_ref == self.invocation_ref && chunks == self.chunks => {
                self.complete = true;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "adapter context stream identity or sequence mismatch",
                ))
            }
        }
        Ok(())
    }
}

impl<R: BufRead> Read for ContextStreamReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.pending.read(buffer)?;
            if count > 0 || self.complete {
                return Ok(count);
            }
            self.next_chunk()?;
        }
    }
}

struct StopAwareWriter<'a, W> {
    writer: &'a mut W,
    stop: &'a ProcessAdapterStopControl,
}

impl<W: Write> Write for StopAwareWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.stop.is_requested() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "adapter stopped"));
        }
        self.writer.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

fn write_context_unless_stopped(
    stop: &ProcessAdapterStopControl,
    writer: &mut impl Write,
    invocation_ref: &str,
    context: &Value,
) -> CwrResult<Option<AdapterContextDeliveryMetrics>> {
    let result = write_context_stream(
        &mut StopAwareWriter { writer, stop },
        invocation_ref,
        context,
    );
    if stop.is_requested() {
        return Ok(None);
    }
    result.map(Some)
}
