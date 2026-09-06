use std::io::{Cursor, Seek, SeekFrom};

use xberg::{
    Engine, ExtractionConfig, ExtractionEvent, ExtractionSink, ReaderInput, ReaderStartPosition, SourceDescriptor,
    SourceGeometry,
};

#[derive(Default)]
struct RecordingSink {
    events: Vec<ExtractionEvent>,
}

impl ExtractionSink for RecordingSink {
    fn emit(&mut self, event: ExtractionEvent) -> xberg::Result<()> {
        self.events.push(event);
        Ok(())
    }
}

#[test]
fn reader_extraction_uses_the_callers_current_capability_without_a_path_reopen() {
    let mut source = Cursor::new(b"ignore\nfirst\nsecond".to_vec());
    source.seek(SeekFrom::Start(7)).unwrap();
    let input = ReaderInput::new(
        &mut source,
        SourceDescriptor::new("text/plain").with_filename("retained.txt"),
    )
    .with_start_position(ReaderStartPosition::Current)
    .with_max_fragment_bytes(5)
    .unwrap();
    let mut sink = RecordingSink::default();

    Engine::new_default()
        .extract_reader(input, &ExtractionConfig::default(), &mut sink)
        .unwrap();

    let text_events: Vec<_> = sink
        .events
        .iter()
        .filter_map(|event| match event {
            ExtractionEvent::Text(fragment) => Some(fragment),
            _ => None,
        })
        .collect();
    assert!(!text_events.is_empty());
    assert!(text_events.iter().all(|fragment| fragment.text.len() <= 5));
    assert_eq!(
        text_events
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<String>(),
        "first\nsecond"
    );
    assert_eq!(
        text_events.first().unwrap().source,
        SourceGeometry::ExactRawByteSpan { start: 7, end: 12 }
    );
    assert_eq!(
        text_events.last().unwrap().source,
        SourceGeometry::ExactRawByteSpan { start: 17, end: 19 }
    );
    assert!(matches!(sink.events.last(), Some(ExtractionEvent::Complete(_))));
}

#[test]
fn reader_extraction_honors_a_preexisting_cancellation_request_before_reading() {
    let token = xberg::cancellation::CancellationToken::new();
    token.cancel();
    let config = ExtractionConfig {
        cancel_token: Some(token),
        ..ExtractionConfig::default()
    };
    let mut source = Cursor::new(b"must not be delivered".to_vec());
    source.seek(SeekFrom::Start(7)).unwrap();
    let input = ReaderInput::new(&mut source, SourceDescriptor::new("text/plain"));
    let mut sink = RecordingSink::default();

    let error = Engine::new_default()
        .extract_reader(input, &config, &mut sink)
        .expect_err("a cancelled reader operation must not read or emit source content");

    assert!(matches!(error, xberg::XbergError::Cancelled));
    assert!(sink.events.is_empty());
    assert_eq!(
        source.stream_position().unwrap(),
        7,
        "cancellation must precede any cursor mutation"
    );
}

#[test]
fn reader_extraction_rejects_a_retained_capability_that_disagrees_with_its_declared_length() {
    let mut source = Cursor::new(b"nineteen-byte-source".to_vec());
    let input = ReaderInput::new(&mut source, SourceDescriptor::new("text/plain").with_length(18));
    let mut sink = RecordingSink::default();

    let error = Engine::new_default()
        .extract_reader(input, &ExtractionConfig::default(), &mut sink)
        .expect_err("the caller-supplied source length must bind the retained reader");

    assert!(matches!(error, xberg::XbergError::Validation { .. }));
    assert!(
        sink.events.is_empty(),
        "a length mismatch must fail before any sink event"
    );
}

#[test]
fn reader_input_rejects_fragment_windows_larger_than_the_contract_limit() {
    let mut source = Cursor::new(Vec::new());

    let error =
        match ReaderInput::new(&mut source, SourceDescriptor::new("text/plain")).with_max_fragment_bytes(1_048_577) {
            Ok(_) => panic!("a reader fragment window above one MiB must fail closed"),
            Err(error) => error,
        };

    assert!(matches!(error, xberg::XbergError::Validation { .. }));
}

#[test]
fn reader_extraction_keeps_utf8_boundary_fragments_within_the_declared_window() {
    let mut source = Cursor::new("ab€wxyz".as_bytes().to_vec());
    let input = ReaderInput::new(&mut source, SourceDescriptor::new("text/plain"))
        .with_max_fragment_bytes(4)
        .unwrap();
    let mut sink = RecordingSink::default();

    Engine::new_default()
        .extract_reader(input, &ExtractionConfig::default(), &mut sink)
        .unwrap();

    let text_events: Vec<_> = sink
        .events
        .iter()
        .filter_map(|event| match event {
            ExtractionEvent::Text(fragment) => Some(fragment),
            _ => None,
        })
        .collect();
    assert_eq!(
        text_events
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<String>(),
        "ab€wxyz"
    );
    assert!(text_events.iter().all(|fragment| fragment.text.len() <= 4));
    assert_eq!(
        text_events
            .iter()
            .map(|fragment| match fragment.source {
                SourceGeometry::ExactRawByteSpan { start, end } => (start, end),
                _ => panic!("retained text fragments must retain exact raw byte spans"),
            })
            .collect::<Vec<_>>(),
        vec![(0, 2), (2, 6), (6, 8), (8, 9)]
    );
}

fn logical_range_tuples(events: &[ExtractionEvent]) -> Vec<(u64, u64, u64, u64, u64, u32, u32)> {
    events
        .iter()
        .filter_map(|event| match event {
            ExtractionEvent::LogicalRange(range) => Some((
                range.sequence,
                range.raw_byte_start,
                range.raw_byte_end,
                range.codepoint_start,
                range.codepoint_end,
                range.line_start,
                range.line_end,
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn reader_extraction_emits_unicode_paragraph_ranges_with_hybrid_whitespace_and_crlf_coordinates() {
    let mut source = Cursor::new(" \u{001c}é\r\n中\n\nZ ".as_bytes().to_vec());
    let input = ReaderInput::new(&mut source, SourceDescriptor::new("text/plain"))
        .with_max_fragment_bytes(4)
        .unwrap();
    let mut sink = RecordingSink::default();

    Engine::new_default()
        .extract_reader(input, &ExtractionConfig::default(), &mut sink)
        .unwrap();

    assert_eq!(
        logical_range_tuples(&sink.events),
        vec![(0, 2, 9, 2, 6, 1, 2), (1, 11, 12, 8, 9, 4, 4)]
    );
}

#[test]
fn reader_extraction_emits_ranges_across_utf8_fragment_boundaries() {
    let mut source = Cursor::new("ab€cd\n\nef".as_bytes().to_vec());
    let input = ReaderInput::new(&mut source, SourceDescriptor::new("text/plain"))
        .with_max_fragment_bytes(4)
        .unwrap();
    let mut sink = RecordingSink::default();

    Engine::new_default()
        .extract_reader(input, &ExtractionConfig::default(), &mut sink)
        .unwrap();

    assert_eq!(
        logical_range_tuples(&sink.events),
        vec![(0, 0, 7, 0, 5, 1, 1), (1, 9, 11, 7, 9, 3, 3)]
    );
}

#[test]
fn reader_extraction_emits_no_logical_ranges_for_whitespace_only_input() {
    let mut source = Cursor::new(" \u{001c}\r\n\u{3000}\n".as_bytes().to_vec());
    let input = ReaderInput::new(&mut source, SourceDescriptor::new("text/plain"));
    let mut sink = RecordingSink::default();

    Engine::new_default()
        .extract_reader(input, &ExtractionConfig::default(), &mut sink)
        .unwrap();

    assert!(logical_range_tuples(&sink.events).is_empty());
    assert!(matches!(sink.events.last(), Some(ExtractionEvent::Complete(_))));
}

struct CancellingSink {
    token: xberg::cancellation::CancellationToken,
    events: Vec<ExtractionEvent>,
}

impl ExtractionSink for CancellingSink {
    fn emit(&mut self, event: ExtractionEvent) -> xberg::Result<()> {
        if matches!(event, ExtractionEvent::Text(_)) {
            self.token.cancel();
        }
        self.events.push(event);
        Ok(())
    }
}

#[test]
fn reader_extraction_stops_before_logical_range_delivery_when_cancelled_by_a_text_event() {
    let token = xberg::cancellation::CancellationToken::new();
    let config = ExtractionConfig {
        cancel_token: Some(token.clone()),
        ..ExtractionConfig::default()
    };
    let mut source = Cursor::new(b"first\n\nsecond".to_vec());
    let input = ReaderInput::new(&mut source, SourceDescriptor::new("text/plain"));
    let mut sink = CancellingSink {
        token,
        events: Vec::new(),
    };

    let error = Engine::new_default()
        .extract_reader(input, &config, &mut sink)
        .expect_err("cancellation must stop range delivery");

    assert!(matches!(error, xberg::XbergError::Cancelled));
    assert!(matches!(sink.events.as_slice(), [ExtractionEvent::Text(_)]));
}
