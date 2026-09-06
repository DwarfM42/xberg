//! Caller-retained reader extraction contract.
//!
//! This Rust-only surface accepts a caller-owned `Read + Seek` capability and
//! delivers bounded extraction events to a caller-owned sink. It never accepts a
//! pathname, so it cannot reopen a mutable external source by accident. The
//! legacy bytes/URI APIs remain unchanged compatibility collectors.

use std::io::{Read, Seek, SeekFrom};

use crate::{Result, XbergError};

const MIN_READER_FRAGMENT_BYTES: usize = 4;
const MAX_READER_FRAGMENT_BYTES: usize = 1024 * 1024;

/// Object-safe capability required by [`ReaderInput`].
pub trait Reader: Read + Seek {}

impl<T: Read + Seek + ?Sized> Reader for T {}

/// Explicit cursor behavior for a caller-retained source capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderStartPosition {
    /// Reset the capability to raw source byte zero before extraction.
    ResetToStart,
    /// Begin at the caller's current cursor and retain absolute raw byte offsets.
    Current,
}

/// Caller-supplied source facts. This descriptor intentionally does not contain
/// a filesystem path or URI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDescriptor {
    /// MIME type selected by the caller's admission boundary.
    pub mime_type: String,
    /// Opaque display name useful for diagnostics and format hints.
    pub filename: Option<String>,
    /// Optional caller-observed source length. It is descriptive; the reader is
    /// the only byte authority.
    pub length: Option<u64>,
}

impl SourceDescriptor {
    /// Construct a source descriptor from an already-selected MIME type.
    pub fn new(mime_type: impl Into<String>) -> Self {
        Self {
            mime_type: mime_type.into(),
            filename: None,
            length: None,
        }
    }

    /// Add a display-only filename hint without authorizing any path access.
    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }

    /// Add a caller-observed length fact without changing byte authority.
    pub fn with_length(mut self, length: u64) -> Self {
        self.length = Some(length);
        self
    }
}

/// Input for the retained-reader extraction surface.
pub struct ReaderInput<'a> {
    reader: &'a mut dyn Reader,
    descriptor: SourceDescriptor,
    start_position: ReaderStartPosition,
    max_fragment_bytes: usize,
}

impl<'a> ReaderInput<'a> {
    /// Construct a reader input with a conservative bounded event window.
    pub fn new(reader: &'a mut dyn Reader, descriptor: SourceDescriptor) -> Self {
        Self {
            reader,
            descriptor,
            start_position: ReaderStartPosition::ResetToStart,
            max_fragment_bytes: 64 * 1024,
        }
    }

    /// Select whether extraction resets the source or begins at the current cursor.
    pub fn with_start_position(mut self, start_position: ReaderStartPosition) -> Self {
        self.start_position = start_position;
        self
    }

    /// Set the maximum UTF-8 text bytes delivered in one sink event.
    ///
    /// This controls only the bounded delivery window, never a document-size
    /// policy. The caller owns the chosen bound and may keep consuming events
    /// until the retained source is exhausted.
    pub fn with_max_fragment_bytes(mut self, max_fragment_bytes: usize) -> Result<Self> {
        if !(MIN_READER_FRAGMENT_BYTES..=MAX_READER_FRAGMENT_BYTES).contains(&max_fragment_bytes) {
            return Err(XbergError::validation(
                "reader fragment bound must be between four bytes and one MiB",
            ));
        }
        self.max_fragment_bytes = max_fragment_bytes;
        Ok(self)
    }

    pub(crate) fn into_parts(self) -> (&'a mut dyn Reader, SourceDescriptor, ReaderStartPosition, usize) {
        (
            self.reader,
            self.descriptor,
            self.start_position,
            self.max_fragment_bytes,
        )
    }
}

/// Honest source geometry attached to an extraction event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceGeometry {
    /// Exact offsets in the raw bytes read from the caller's capability.
    ExactRawByteSpan { start: u64, end: u64 },
    /// A format-native transformed or rendered coordinate system.
    Derived,
    /// No meaningful source geometry is available from the extractor.
    Unavailable,
}

/// One bounded text fragment delivered by the reader extraction surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextFragment {
    pub text: String,
    pub source: SourceGeometry,
}

/// Terminal status for a completed reader extraction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractionComplete {
    /// Raw byte cursor after the final consumed source byte.
    pub source_end: u64,
}

/// Bounded event emitted by retained-reader extraction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtractionEvent {
    Text(TextFragment),
    Complete(ExtractionComplete),
}

/// Caller-owned bounded output consumer.
pub trait ExtractionSink {
    /// Receive one bounded extraction event. Returning an error stops extraction
    /// immediately and preserves the sink's error classification.
    fn emit(&mut self, event: ExtractionEvent) -> Result<()>;
}

/// Input behavior for one format in the reader contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderInputCapability {
    RetainedSourceCompatible,
    BufferedWholeInputRequired,
    ExternalPathDependency,
    CompanionFilesRequired,
}

/// Output behavior for one format in the reader contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderOutputCapability {
    BoundedIncrementalOutput,
    WholeOutputMaterializationRequired,
}

/// Provenance behavior for one format in the reader contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceProvenanceCapability {
    ExactRawSourceByteSpans,
    DerivedGeometryOnly,
    Unavailable,
}

/// Typed snapshot used by hosts to fail closed before provider selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReaderFormatCapabilities {
    pub mime_type: &'static str,
    pub input: ReaderInputCapability,
    pub output: ReaderOutputCapability,
    pub provenance: SourceProvenanceCapability,
    pub requires_ocr_or_model: bool,
    pub requires_native_runtime: bool,
}

/// Report the exact reader-contract state known for a MIME type.
///
/// `None` means this contract has not qualified the format; callers must not
/// infer safety from the legacy Xberg format registry.
pub fn reader_format_capabilities(mime_type: &str) -> Option<ReaderFormatCapabilities> {
    match mime_type {
        "text/plain" => Some(ReaderFormatCapabilities {
            mime_type: "text/plain",
            input: ReaderInputCapability::RetainedSourceCompatible,
            output: ReaderOutputCapability::BoundedIncrementalOutput,
            provenance: SourceProvenanceCapability::ExactRawSourceByteSpans,
            requires_ocr_or_model: false,
            requires_native_runtime: false,
        }),
        _ => None,
    }
}

pub(crate) fn extract_reader(
    input: ReaderInput<'_>,
    config: &crate::ExtractionConfig,
    sink: &mut dyn ExtractionSink,
) -> Result<()> {
    let (reader, descriptor, start_position, max_fragment_bytes) = input.into_parts();
    if reader_format_capabilities(&descriptor.mime_type).is_none() {
        return Err(XbergError::UnsupportedFormat(descriptor.mime_type));
    }
    if config.cancel_token.as_ref().is_some_and(|token| token.is_cancelled()) {
        return Err(XbergError::Cancelled);
    }
    let expected_length = descriptor.length;
    let mut offset = match start_position {
        ReaderStartPosition::ResetToStart => reader.seek(SeekFrom::Start(0))?,
        ReaderStartPosition::Current => reader.stream_position()?,
    };
    if let Some(expected_length) = expected_length {
        let observed_length = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(offset))?;
        if observed_length != expected_length {
            return Err(XbergError::validation(
                "retained reader length differs from the caller-supplied source fact",
            ));
        }
    }
    let mut pending = Vec::with_capacity(max_fragment_bytes.saturating_add(3));
    let mut buffer = vec![0_u8; max_fragment_bytes];

    loop {
        if config.cancel_token.as_ref().is_some_and(|token| token.is_cancelled()) {
            return Err(XbergError::Cancelled);
        }
        let count = reader.read(&mut buffer)?;
        if config.cancel_token.as_ref().is_some_and(|token| token.is_cancelled()) {
            return Err(XbergError::Cancelled);
        }
        if count == 0 {
            if !pending.is_empty() {
                emit_valid_utf8(&pending, offset, max_fragment_bytes, config, sink)?;
                offset = offset
                    .checked_add(pending.len() as u64)
                    .ok_or_else(|| XbergError::validation("reader source offset overflowed"))?;
            }
            break;
        }
        pending.extend_from_slice(&buffer[..count]);
        let consumed = emit_complete_utf8_prefix(&pending, offset, max_fragment_bytes, config, sink)?;
        if consumed > 0 {
            offset = offset
                .checked_add(consumed as u64)
                .ok_or_else(|| XbergError::validation("reader source offset overflowed"))?;
            pending.drain(..consumed);
        }
        if pending.len() > 3 {
            return Err(XbergError::parsing("reader source is not valid UTF-8"));
        }
    }

    if expected_length.is_some_and(|expected| expected != offset) {
        return Err(XbergError::validation(
            "retained reader length differs from the caller-supplied source fact",
        ));
    }
    if config.cancel_token.as_ref().is_some_and(|token| token.is_cancelled()) {
        return Err(XbergError::Cancelled);
    }
    sink.emit(ExtractionEvent::Complete(ExtractionComplete { source_end: offset }))
}

fn emit_complete_utf8_prefix(
    bytes: &[u8],
    offset: u64,
    max_fragment_bytes: usize,
    config: &crate::ExtractionConfig,
    sink: &mut dyn ExtractionSink,
) -> Result<usize> {
    match std::str::from_utf8(bytes) {
        Ok(_) => {
            emit_valid_utf8(bytes, offset, max_fragment_bytes, config, sink)?;
            Ok(bytes.len())
        }
        Err(error) if error.error_len().is_none() => {
            let valid = error.valid_up_to();
            if valid > 0 {
                emit_valid_utf8(&bytes[..valid], offset, max_fragment_bytes, config, sink)?;
            }
            Ok(valid)
        }
        Err(_) => Err(XbergError::parsing("reader source is not valid UTF-8")),
    }
}

fn emit_valid_utf8(
    bytes: &[u8],
    start: u64,
    max_fragment_bytes: usize,
    config: &crate::ExtractionConfig,
    sink: &mut dyn ExtractionSink,
) -> Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| XbergError::parsing("reader source is not valid UTF-8"))?;
    let mut emitted = 0_usize;
    while emitted < bytes.len() {
        if config.cancel_token.as_ref().is_some_and(|token| token.is_cancelled()) {
            return Err(XbergError::Cancelled);
        }
        let candidate_end = (emitted + max_fragment_bytes).min(bytes.len());
        let mut end = candidate_end;
        while !text.is_char_boundary(end) {
            end = end
                .checked_sub(1)
                .ok_or_else(|| XbergError::validation("reader fragment bound splits a UTF-8 scalar"))?;
        }
        if end <= emitted {
            return Err(XbergError::validation("reader fragment bound splits a UTF-8 scalar"));
        }
        let fragment_start = start
            .checked_add(emitted as u64)
            .ok_or_else(|| XbergError::validation("reader source offset overflowed"))?;
        let fragment_end = start
            .checked_add(end as u64)
            .ok_or_else(|| XbergError::validation("reader source offset overflowed"))?;
        sink.emit(ExtractionEvent::Text(TextFragment {
            text: text[emitted..end].to_owned(),
            source: SourceGeometry::ExactRawByteSpan {
                start: fragment_start,
                end: fragment_end,
            },
        }))?;
        emitted = end;
    }
    Ok(())
}
