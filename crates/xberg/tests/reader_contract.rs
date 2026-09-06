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
    assert!(!matches!(sink.events.last(), Some(ExtractionEvent::Complete(_))));
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
            ExtractionEvent::Complete(_) => None,
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
