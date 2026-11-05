use std::io::{self, BufRead, Read};

pub(crate) const MAX_JSONL_LINE_BYTES: usize = 1024 * 1024;

#[cfg(test)]
pub(crate) fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    read_line_with_termination(reader, false)
}

pub(crate) fn read_delimited_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    read_line_with_termination(reader, true)
}

pub(crate) fn read_delimited_bytes(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    read_bytes_with_termination(reader, true)
}

fn read_line_with_termination(
    reader: &mut impl BufRead,
    require_newline: bool,
) -> io::Result<Option<String>> {
    read_bytes_with_termination(reader, require_newline)?.map_or(Ok(None), decode_line)
}

fn read_bytes_with_termination(
    reader: &mut impl BufRead,
    require_newline: bool,
) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let mut limited = reader.take((MAX_JSONL_LINE_BYTES + 1) as u64);
    if limited.read_until(b'\n', &mut bytes)? == 0 {
        return Ok(None);
    }
    if bytes.len() > MAX_JSONL_LINE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("JSONL frame exceeds the {MAX_JSONL_LINE_BYTES}-byte limit"),
        ));
    }
    if require_newline && bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "host connection closed with an incomplete JSONL frame after {} bytes",
                bytes.len()
            ),
        ));
    }
    Ok(Some(bytes))
}

#[cfg(test)]
pub(crate) fn read_unbounded_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    read_unbounded_line_with_termination(reader, false)
}

pub(crate) fn read_unbounded_delimited_line(
    reader: &mut impl BufRead,
) -> io::Result<Option<String>> {
    read_unbounded_line_with_termination(reader, true)
}

fn read_unbounded_line_with_termination(
    reader: &mut impl BufRead,
    require_newline: bool,
) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    if reader.read_until(b'\n', &mut bytes)? == 0 {
        return Ok(None);
    }
    if require_newline && bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "runtime service connection closed with an incomplete JSONL frame after {} bytes",
                bytes.len()
            ),
        ));
    }
    decode_line(bytes)
}

fn decode_line(mut bytes: Vec<u8>) -> io::Result<Option<String>> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    String::from_utf8(bytes).map(Some).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("JSONL frame is not UTF-8: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn accepts_a_frame_at_the_limit() {
        let mut frame = vec![b'a'; MAX_JSONL_LINE_BYTES - 1];
        frame.push(b'\n');
        let line = read_line(&mut Cursor::new(frame)).unwrap().unwrap();
        assert_eq!(line.len(), MAX_JSONL_LINE_BYTES - 1);
    }

    #[test]
    fn rejects_a_frame_over_the_limit_without_allocating_the_remainder() {
        let frame = vec![b'a'; MAX_JSONL_LINE_BYTES + 4096];
        let error = read_line(&mut Cursor::new(frame)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    fn accepts_a_final_frame_without_a_newline() {
        let line = read_line(&mut Cursor::new(b"{}".to_vec()))
            .unwrap()
            .unwrap();
        assert_eq!(line, "{}");
    }

    #[test]
    fn delimited_reader_accepts_frames_across_the_8192_byte_boundary() {
        for frame_bytes in [8191, 8192, 8193, 16_385] {
            let mut frame = vec![b'x'; frame_bytes];
            frame[0] = b'"';
            frame[frame_bytes - 2] = b'"';
            frame[frame_bytes - 1] = b'\n';
            let mut reader = io::BufReader::with_capacity(8192, Cursor::new(frame));

            let line = read_delimited_line(&mut reader).unwrap().unwrap();

            assert_eq!(line.len(), frame_bytes - 1);
            assert!(serde_json::from_str::<serde_json::Value>(&line).is_ok());
        }
    }

    #[test]
    fn delimited_reader_identifies_an_8192_byte_partial_frame() {
        let error = read_delimited_line(&mut Cursor::new(vec![b'x'; 8192])).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(
            error.to_string(),
            "host connection closed with an incomplete JSONL frame after 8192 bytes"
        );
    }

    #[test]
    fn unbounded_reader_accepts_a_trusted_internal_frame_over_the_host_limit() {
        let mut frame = vec![b'a'; MAX_JSONL_LINE_BYTES + 1];
        frame.push(b'\n');
        let line = read_unbounded_line(&mut Cursor::new(frame))
            .unwrap()
            .unwrap();
        assert_eq!(line.len(), MAX_JSONL_LINE_BYTES + 1);
    }

    #[test]
    fn unbounded_delimited_reader_identifies_a_partial_runtime_frame() {
        let error = read_unbounded_delimited_line(&mut Cursor::new(vec![b'x'; 8192])).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(
            error.to_string(),
            "runtime service connection closed with an incomplete JSONL frame after 8192 bytes"
        );
    }
}
