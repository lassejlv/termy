use super::*;
use crate::CursorStyle;
use std::sync::atomic::{AtomicU64, Ordering};

struct TestTempPath {
    path: PathBuf,
}

impl TestTempPath {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        loop {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "termy-tmon-graphics-test-{}-{id}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self { path },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("failed to create test temp path: {error}"),
            }
        }
    }

    fn file(&self, name: &str, contents: &[u8]) -> PathBuf {
        let path = self.path.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TestTempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn test_size() -> Size {
    Size {
        cols: 80,
        rows: 24,
        cell_width: 10.0,
        cell_height: 20.0,
    }
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in input.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(TABLE[((value >> 18) & 63) as usize] as char);
        output.push(TABLE[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn decode_hex(input: &str) -> Vec<u8> {
    input
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let hex = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(hex, 16).unwrap()
        })
        .collect()
}

fn command(control: &str, payload: &[u8]) -> GraphicsCommand {
    GraphicsCommand::parse(
        format!("{control};{}", encode_base64(payload)).into_bytes(),
        false,
    )
}

fn png_document(
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace: u8,
    chunks: Vec<([u8; 4], Vec<u8>)>,
) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[bit_depth, color_type, 0, 0, interlace]);
    push_png_chunk(&mut png, *b"IHDR", &header);
    for (kind, payload) in chunks {
        push_png_chunk(&mut png, kind, &payload);
    }
    push_png_chunk(&mut png, *b"IEND", &[]);
    png
}

fn png_with_filtered_data(
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace: u8,
    filtered: &[u8],
) -> Vec<u8> {
    png_document(
        width,
        height,
        bit_depth,
        color_type,
        interlace,
        vec![(*b"IDAT", zlib_store(filtered))],
    )
}

fn png_chunks(data: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    let mut chunks = Vec::new();
    let mut offset = 8usize;
    while offset < data.len() {
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = data[offset + 4..offset + 8].try_into().unwrap();
        let payload_start = offset + 8;
        let payload_end = payload_start + length;
        chunks.push((kind, data[payload_start..payload_end].to_vec()));
        offset = payload_end + 4;
    }
    chunks
}

fn buffered_reference_png(width: u32, height: u32, channels: u8, pixels: &[u8]) -> Vec<u8> {
    let row_bytes = width as usize * usize::from(channels);
    let mut filtered = Vec::with_capacity(pixels.len() + height as usize);
    for row in pixels.chunks_exact(row_bytes) {
        filtered.push(0);
        filtered.extend_from_slice(row);
    }
    let compressed = zlib_store(&filtered);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = [0; 13];
    header[..4].copy_from_slice(&width.to_be_bytes());
    header[4..8].copy_from_slice(&height.to_be_bytes());
    header[8] = 8;
    header[9] = if channels == 3 { 2 } else { 6 };
    push_png_chunk(&mut png, *b"IHDR", &header);
    push_png_chunk(&mut png, *b"IDAT", &compressed);
    push_png_chunk(&mut png, *b"IEND", &[]);
    png
}

fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_str()
        .expect("test temp path should be UTF-8")
        .as_bytes()
        .to_vec()
}

fn upload_one_pixel(state: &mut GraphicsState, grid: &mut Grid, image_id: u32) {
    let result = state.apply(
        command(
            &format!("a=t,f=32,s=1,v=1,i={image_id},q=1"),
            &[255, 0, 0, 255],
        ),
        grid,
        test_size(),
    );
    assert!(result.changed);
}

#[test]
fn command_parse_keeps_payload_in_the_parser_allocation() {
    let mut bytes = Vec::with_capacity(128);
    bytes.extend_from_slice(b"a=t,i=7;QUJDRA==");
    let original_pointer = bytes.as_ptr();
    let original_capacity = bytes.capacity();

    let command = GraphicsCommand::parse(bytes, false);

    assert_eq!(command.value('a'), Some("t"));
    assert_eq!(command.value('i'), Some("7"));
    assert_eq!(command.payload, b"QUJDRA==");
    assert_eq!(command.payload.as_ptr(), original_pointer);
    assert_eq!(command.payload.capacity(), original_capacity);
}

#[test]
fn command_parse_caps_the_control_prefix_at_4096_bytes() {
    let mut at_limit = b"i=23,".to_vec();
    at_limit.resize(MAX_CONTROL_BYTES, b'x');
    at_limit.push(b';');
    at_limit.extend_from_slice(b"QUJDRA==");
    let command = GraphicsCommand::parse(at_limit, false);
    assert!(!command.oversized);
    assert_eq!(command.value('i'), Some("23"));
    assert_eq!(command.payload, b"QUJDRA==");

    let mut beyond_limit = b"i=24,".to_vec();
    beyond_limit.resize(MAX_CONTROL_BYTES + 1, b'x');
    beyond_limit.push(b';');
    beyond_limit.extend_from_slice(b"QUJDRA==");
    let command = GraphicsCommand::parse(beyond_limit, false);
    assert!(command.oversized);
    assert_eq!(command.value('i'), Some("24"));
    assert!(command.payload.is_empty());
}

#[test]
fn command_parse_caps_raw_fields_before_decoding_them() {
    let mut bytes = Vec::new();
    for index in 0..=MAX_CONTROL_FIELDS {
        if index > 0 {
            bytes.push(b',');
        }
        bytes.extend_from_slice(b"q=1");
    }
    bytes.extend_from_slice(b",i=\xff;QUJDRA==");

    let command = GraphicsCommand::parse(bytes, false);

    assert!(command.oversized);
    assert_eq!(command.control.len(), MAX_CONTROL_FIELDS);
    assert_eq!(command.value('q'), Some("1"));
    assert_eq!(command.value('i'), None);
    assert!(command.payload.is_empty());

    let externally_oversized = GraphicsCommand::parse(b"i=25;QUJDRA==".to_vec(), true);
    assert_eq!(externally_oversized.value('i'), Some("25"));
    assert!(externally_oversized.payload.is_empty());
}

#[test]
fn pending_upload_retains_metadata_but_not_base64_payload() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);

    let result = state.apply(
        GraphicsCommand::parse(b"a=t,f=32,s=1,v=1,i=7,m=1;AQI=".to_vec(), false),
        &mut grid,
        test_size(),
    );

    assert!(result.replies.is_empty());
    let pending = state.pending.as_ref().unwrap();
    assert_eq!(pending.command.value('i'), Some("7"));
    assert!(pending.command.payload.is_empty());
    assert_eq!(pending.decoded, [1, 2]);
}

#[test]
fn upload_and_stored_image_limits_are_distinct() {
    assert_eq!(MAX_UPLOAD_BYTES, 64 * 1024 * 1024);
    assert_eq!(MAX_STORED_IMAGE_BYTES, 2 * MAX_UPLOAD_BYTES);
    assert_eq!(MAX_STORED_IMAGES, MAX_PLACEMENTS);
    assert_eq!(
        MAX_COMMAND_BYTES,
        MAX_CONTROL_BYTES + 1 + MAX_UPLOAD_BYTES.div_ceil(3) * 4
    );
    assert_eq!(
        validate_dimensions(4096, 4096, 4).unwrap(),
        MAX_UPLOAD_BYTES
    );
    assert!(validate_dimensions(4097, 4096, 4).is_err());

    let (scanlines, _, _) = png_scanline_layout(PngHeader {
        width: 4096,
        height: 4096,
        bit_depth: 8,
        color_type: 6,
        interlace: 0,
    })
    .unwrap();
    assert_eq!(scanlines, MAX_UPLOAD_BYTES + 4096);

    let mut state = GraphicsState {
        stored_bytes: MAX_UPLOAD_BYTES + 1,
        ..GraphicsState::default()
    };
    assert!(state.enforce_quota(None));
    state.stored_bytes = MAX_STORED_IMAGE_BYTES + 1;
    assert!(!state.enforce_quota(None));
}

#[test]
fn stored_image_count_is_bounded_and_evicts_oldest_unplaced_image() {
    let mut state = GraphicsState::default();
    let pixel: Arc<[u8]> = Arc::from([0_u8]);
    let protected = u32::try_from(MAX_STORED_IMAGES + 1).unwrap();

    for image_id in 1..=protected {
        state.images.insert(
            image_id,
            StoredImage {
                png: pixel.clone(),
                width: 1,
                height: 1,
                number: None,
                generation: u64::from(image_id),
            },
        );
        state.insertion_order.push_back(image_id);
        state.stored_bytes += 1;
    }

    assert!(state.enforce_quota(Some(protected)));
    assert_eq!(state.images.len(), MAX_STORED_IMAGES);
    assert_eq!(state.insertion_order.len(), MAX_STORED_IMAGES);
    assert_eq!(state.stored_bytes, MAX_STORED_IMAGES);
    assert!(!state.images.contains_key(&1));
    assert!(state.images.contains_key(&protected));
}

#[test]
fn failed_image_replacement_keeps_the_previous_image_and_placements() {
    let size = test_size();
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(size.cols, size.rows, 100, CursorStyle::Block);
    let first = state.apply(
        command("a=T,f=32,s=1,v=1,i=7,c=2,r=1,C=1,q=1", &[255, 0, 0, 255]),
        &mut grid,
        size,
    );
    assert!(first.changed);
    let image = state.images.get(&7).unwrap().clone();
    let placements = state.placements.clone();

    let invalid = state.apply(
        command("a=T,f=32,s=1,v=1,i=7,U=1,C=1,q=1", &[0, 255, 0, 255]),
        &mut grid,
        size,
    );
    assert!(!invalid.changed);
    assert_eq!(state.images.get(&7).unwrap().png, image.png);
    assert_eq!(state.placements.len(), placements.len());
    assert_eq!(state.placements[0].serial, placements[0].serial);

    state.stored_bytes = MAX_STORED_IMAGE_BYTES.saturating_add(image.png.len());
    let quota = state.apply(
        command("a=T,f=32,s=1,v=1,i=7,C=1,q=1", &[0, 0, 255, 255]),
        &mut grid,
        size,
    );
    assert!(!quota.changed);
    assert!(
        String::from_utf8_lossy(&quota.replies).contains("ENOSPC:image storage quota exceeded")
    );
    assert_eq!(state.images.get(&7).unwrap().png, image.png);
    assert_eq!(state.placements.len(), placements.len());
    assert_eq!(state.placements[0].serial, placements[0].serial);
}

#[test]
fn anonymous_image_ids_wrap_without_stalling_at_one() {
    let mut state = GraphicsState {
        next_anonymous_id: 1,
        ..GraphicsState::default()
    };
    state.images.insert(
        1,
        StoredImage {
            png: Arc::from([0_u8]),
            width: 1,
            height: 1,
            number: None,
            generation: 1,
        },
    );

    assert_eq!(state.allocate_anonymous_id(), u32::MAX);
    assert_eq!(state.next_anonymous_id, u32::MAX - 1);
}

#[test]
fn streamed_png_encoding_matches_buffered_bytes_across_blocks() {
    let width = 8192;
    let height = 2;
    let pixels = (0..width as usize * height as usize * 4)
        .map(|index| index as u8)
        .collect::<Vec<_>>();

    let streamed = encode_png(width, height, 4, &pixels);
    let buffered = buffered_reference_png(width, height, 4, &pixels);

    assert_eq!(streamed, buffered);
    assert_eq!(validate_png(&streamed).unwrap(), (width, height));
}

#[test]
fn crc32_table_matches_known_vectors_and_incremental_updates() {
    assert_eq!(crc32_parts(&[b""]), 0);
    assert_eq!(crc32_parts(&[b"123456789"]), 0xcbf4_3926);
    assert_eq!(
        crc32_parts(&[b"123", b"456", b"789"]),
        crc32_parts(&[b"123456789"])
    );
}

#[test]
fn raw_rgba_upload_encodes_png_and_places_at_cursor() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    grid.set_cursor_position(5, 4);
    let payload = encode_base64(&[255, 0, 0, 255]);
    let command = GraphicsCommand::parse(
        format!("a=T,f=32,s=1,v=1,i=42,c=2,r=3;{payload}").into_bytes(),
        false,
    );
    let result = state.apply(command, &mut grid, test_size());

    assert!(result.changed);
    assert_eq!(result.replies, b"\x1b_Gi=42;OK\x1b\\");
    let placements = state.render_placements(&grid);
    assert_eq!(placements.len(), 1);
    assert!(placements[0].png.starts_with(b"\x89PNG"));
    assert_eq!((placements[0].viewport_row, placements[0].col), (5, 4));
}

#[test]
fn image_numbers_allocate_new_ids_and_resolve_only_the_newest_image() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    let first = state.apply(
        command("a=t,f=32,s=1,v=1,I=13", &[1, 2, 3, 255]),
        &mut grid,
        test_size(),
    );
    let second = state.apply(
        command("a=t,f=32,s=1,v=1,I=13", &[4, 5, 6, 255]),
        &mut grid,
        test_size(),
    );

    assert_eq!(first.replies, b"\x1b_Gi=4294967295,I=13;OK\x1b\\");
    assert_eq!(second.replies, b"\x1b_Gi=4294967294,I=13;OK\x1b\\");
    assert_eq!(state.images.len(), 2);

    let placed = state.apply(command("a=p,I=13,p=7,C=1", &[]), &mut grid, test_size());
    assert!(placed.changed);
    assert_eq!(placed.replies, b"\x1b_Gi=4294967294,I=13,p=7;OK\x1b\\");
    assert_eq!(state.placements.len(), 1);
    assert_eq!(state.placements[0].image_id, u32::MAX - 1);
}

#[test]
fn image_id_and_number_together_are_rejected_before_storage() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    let result = state.apply(
        command("a=t,f=32,s=1,v=1,i=7,I=13", &[1, 2, 3, 255]),
        &mut grid,
        test_size(),
    );

    assert!(!result.changed);
    assert_eq!(
        result.replies,
        b"\x1b_Gi=7,I=13;EINVAL:image id and image number are mutually exclusive\x1b\\"
    );
    assert!(state.images.is_empty());
}

#[test]
fn number_delete_targets_newest_and_uppercase_frees_only_when_unreferenced() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    for pixel in [[1, 2, 3, 255], [4, 5, 6, 255]] {
        state.apply(
            command("a=t,f=32,s=1,v=1,I=13,q=1", &pixel),
            &mut grid,
            test_size(),
        );
    }
    let newest = u32::MAX - 1;
    state.apply(command("a=p,I=13,p=7,C=1,q=1", &[]), &mut grid, test_size());

    let soft = state.apply(command("a=d,d=n,I=13,p=7,q=1", &[]), &mut grid, test_size());
    assert!(soft.changed);
    assert!(state.images.contains_key(&newest));
    assert!(state.placements.is_empty());

    let hard = state.apply(command("a=d,d=N,I=13,q=1", &[]), &mut grid, test_size());
    assert!(!hard.changed);
    assert!(!state.images.contains_key(&newest));
    assert_eq!(
        state.resolve_image_id(&command("I=13", &[])),
        Some(u32::MAX)
    );
}

#[test]
fn image_id_range_delete_is_global_and_hard_delete_frees_only_the_range() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    for image_id in 10..=12 {
        state.apply(
            command(
                &format!("a=T,f=32,s=1,v=1,i={image_id},C=1,q=1"),
                &[image_id as u8, 0, 0, 255],
            ),
            &mut grid,
            test_size(),
        );
    }

    let soft = state.apply(
        command("a=d,d=r,x=10,y=11,q=1", &[]),
        &mut grid,
        test_size(),
    );
    assert!(soft.changed);
    assert!(state.images.contains_key(&10));
    assert!(state.images.contains_key(&11));
    assert!(state.images.contains_key(&12));
    assert_eq!(state.placements.len(), 1);
    assert_eq!(state.placements[0].image_id, 12);

    let hard = state.apply(
        command("a=d,d=R,x=10,y=11,q=1", &[]),
        &mut grid,
        test_size(),
    );
    assert!(!hard.changed);
    assert!(!state.images.contains_key(&10));
    assert!(!state.images.contains_key(&11));
    assert!(state.images.contains_key(&12));
    assert_eq!(state.placements.len(), 1);
}

#[test]
fn uppercase_coordinate_delete_does_not_free_unrelated_soft_images() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    state.apply(
        command("a=t,f=32,s=1,v=1,i=1,q=1", &[1, 1, 1, 255]),
        &mut grid,
        test_size(),
    );
    state.apply(
        command("a=T,f=32,s=1,v=1,i=2,C=1,q=1", &[2, 2, 2, 255]),
        &mut grid,
        test_size(),
    );
    grid.set_cursor_position(4, 4);
    state.apply(
        command("a=T,f=32,s=1,v=1,i=3,C=1,q=1", &[3, 3, 3, 255]),
        &mut grid,
        test_size(),
    );

    let deleted = state.apply(command("a=d,d=P,x=1,y=1,q=1", &[]), &mut grid, test_size());

    assert!(deleted.changed);
    assert!(state.images.contains_key(&1));
    assert!(!state.images.contains_key(&2));
    assert!(state.images.contains_key(&3));
    assert_eq!(state.placements.len(), 1);
    assert_eq!(state.placements[0].image_id, 3);
}

#[test]
fn continuation_failures_reply_with_the_original_image_id() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    let first = GraphicsCommand::parse(b"a=t,f=32,s=1,v=1,i=7,m=1;AQI=".to_vec(), false);
    assert!(
        state
            .apply(first, &mut grid, test_size())
            .replies
            .is_empty()
    );

    let malformed = GraphicsCommand::parse(b"m=0;!".to_vec(), false);
    assert_eq!(
        state.apply(malformed, &mut grid, test_size()).replies,
        b"\x1b_Gi=7;EINVAL:invalid base64 payload\x1b\\"
    );

    let first = GraphicsCommand::parse(b"a=t,f=32,s=1,v=1,i=8,m=1;AQI=".to_vec(), false);
    assert!(
        state
            .apply(first, &mut grid, test_size())
            .replies
            .is_empty()
    );
    let oversized = GraphicsCommand::parse(b"m=0".to_vec(), true);
    assert_eq!(
        state.apply(oversized, &mut grid, test_size()).replies,
        b"\x1b_Gi=8;EFBIG:image command exceeds storage limit\x1b\\"
    );
}

#[test]
fn continuation_rejects_new_upload_controls_and_discards_pending_data() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    let first = GraphicsCommand::parse(b"a=t,f=32,s=1,v=1,i=7,m=1;AQI=".to_vec(), false);
    assert!(
        state
            .apply(first, &mut grid, test_size())
            .replies
            .is_empty()
    );

    let invalid = GraphicsCommand::parse(b"i=8,m=0;A/8=".to_vec(), false);
    assert_eq!(
        state.apply(invalid, &mut grid, test_size()).replies,
        b"\x1b_Gi=7;EINVAL:continuation contains unsupported control data\x1b\\"
    );
    assert!(state.pending.is_none());
    assert!(state.images.is_empty());
}

#[test]
fn chunked_placement_does_not_advance_a_different_screen() {
    let size = Size {
        cols: 8,
        rows: 4,
        ..Size::default()
    };
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(size.cols, size.rows, 100, CursorStyle::Block);
    grid.set_cursor_position(1, 2);
    let first =
        GraphicsCommand::parse(b"a=T,f=32,s=1,v=1,i=7,c=2,r=1,m=1,q=1;AQI=".to_vec(), false);
    assert!(!state.apply(first, &mut grid, size).changed);

    grid.set_mode(true, 1049, true);
    let alternate_cursor = grid.cursor_position();
    let final_chunk = GraphicsCommand::parse(b"m=0;A/8=".to_vec(), false);
    assert!(state.apply(final_chunk, &mut grid, size).changed);
    assert_eq!(grid.cursor_position(), alternate_cursor);

    grid.set_mode(true, 1049, false);
    let placements = state.render_placements(&grid);
    assert_eq!(placements.len(), 1);
    assert_eq!((placements[0].viewport_row, placements[0].col), (1, 2));
}

#[test]
fn terminal_reset_clears_placements_but_keeps_image_data() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    upload_one_pixel(&mut state, &mut grid, 7);
    let placed = state.apply(command("a=p,i=7,C=1,q=1", &[]), &mut grid, test_size());
    assert!(placed.changed);
    assert_eq!(state.render_placements(&grid).len(), 1);

    assert!(state.apply_grid_effect(GridEffect::Reset));
    assert!(state.render_placements(&grid).is_empty());
    assert!(state.images.contains_key(&7));

    let reused = state.apply(command("a=p,i=7,C=1,q=1", &[]), &mut grid, test_size());
    assert!(reused.changed);
    assert_eq!(state.render_placements(&grid).len(), 1);
}

#[test]
fn partial_top_region_scroll_keeps_footer_placement_fixed() {
    let size = Size {
        cols: 8,
        rows: 4,
        ..Size::default()
    };
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(size.cols, size.rows, 100, CursorStyle::Block);
    grid.set_cursor_position(3, 0);
    let placed = state.apply(
        command("a=T,f=32,s=1,v=1,i=9,C=1,q=1", &[1, 2, 3, 255]),
        &mut grid,
        size,
    );
    assert!(placed.changed);
    assert_eq!(state.render_placements(&grid)[0].viewport_row, 3);

    grid.set_scroll_region(0, 2);
    grid.set_cursor_position(2, 0);
    grid.line_feed();
    while let Some(effect) = grid.pop_effect() {
        state.apply_grid_effect(effect);
    }

    assert_eq!(grid.history_size(), 1);
    assert_eq!(state.render_placements(&grid)[0].viewport_row, 3);
}

#[test]
fn post_placement_cursor_advance_does_not_scroll_partial_region() {
    let size = Size {
        cols: 8,
        rows: 4,
        ..Size::default()
    };
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(size.cols, size.rows, 0, CursorStyle::Block);
    grid.set_cursor_position(1, 0);
    grid.put_char('A');
    grid.set_cursor_position(2, 0);
    grid.put_char('B');
    grid.set_scroll_region(1, 2);
    grid.set_cursor_position(2, 0);
    let cells_before = grid.snapshot().cells;

    let result = state.apply(
        command("a=T,f=32,s=1,v=1,i=82,c=2,r=3,q=1", &[1, 2, 3, 255]),
        &mut grid,
        size,
    );

    assert!(result.changed);
    assert_eq!(grid.snapshot().cells, cells_before);
    assert_eq!(grid.cursor_position(), (2, 3));
    while let Some(effect) = grid.pop_effect() {
        state.apply_grid_effect(effect);
    }
    let placements = state.render_placements(&grid);
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].image_id, 82);
    assert_eq!(placements[0].viewport_row, 2);
}

#[test]
fn column_resize_preserves_placements_and_clips_them_at_render_time() {
    let size = Size {
        cols: 8,
        rows: 4,
        ..Size::default()
    };
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(size.cols, size.rows, 8, CursorStyle::Block);
    grid.set_cursor_position(1, 6);
    let result = state.apply(
        command("a=T,f=32,s=1,v=1,i=83,c=2,r=1,C=1,q=1", &[1, 2, 3, 255]),
        &mut grid,
        size,
    );
    assert!(result.changed);
    let initial = state.render_placements(&grid);
    assert_eq!(initial.len(), 1);
    let serial = initial[0].placement_serial;
    let anchor_line = state.placements[0].anchor_line;

    grid.resize(4, 4);
    while let Some(effect) = grid.pop_effect() {
        assert!(!state.apply_grid_effect(effect));
    }

    assert_eq!(state.placements.len(), 1);
    assert_eq!(state.placements[0].anchor_line, anchor_line);
    assert!(state.render_placements(&grid).is_empty());

    grid.resize(8, 4);
    while let Some(effect) = grid.pop_effect() {
        assert!(!state.apply_grid_effect(effect));
    }
    let restored = state.render_placements(&grid);
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].placement_serial, serial);
    assert_eq!((restored[0].viewport_row, restored[0].col), (1, 6));
}

#[test]
fn column_resize_preserves_an_in_progress_chunked_upload() {
    let size = Size {
        cols: 8,
        rows: 4,
        ..Size::default()
    };
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(size.cols, size.rows, 8, CursorStyle::Block);
    grid.set_cursor_position(2, 3);

    let first = state.apply(
        command("a=T,f=32,s=1,v=1,i=84,c=1,r=1,C=1,m=1,q=1", &[1, 2]),
        &mut grid,
        size,
    );
    assert!(!first.changed);
    assert!(state.pending.is_some());

    grid.resize(4, 4);
    while let Some(effect) = grid.pop_effect() {
        assert!(!state.apply_grid_effect(effect));
    }
    assert!(state.pending.is_some());

    let second = state.apply(
        command("m=0,q=1", &[3, 255]),
        &mut grid,
        Size { cols: 4, ..size },
    );
    assert!(second.changed);
    let placements = state.render_placements(&grid);
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].image_id, 84);
    assert_eq!((placements[0].viewport_row, placements[0].col), (2, 3));
}

#[test]
fn explicit_dimensions_keep_full_occupancy_past_the_right_edge() {
    let size = Size {
        cols: 8,
        rows: 4,
        cell_width: 1.0,
        cell_height: 1.0,
    };
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(size.cols, size.rows, 0, CursorStyle::Block);
    let upload = state.apply(
        command("a=t,f=32,s=4,v=1,i=85,q=1", &[255; 16]),
        &mut grid,
        size,
    );
    assert!(upload.changed);

    grid.set_cursor_position(1, 6);
    let placed = state.apply(
        command("a=p,i=85,p=1,c=5,r=2,z=7,C=1,q=1", &[]),
        &mut grid,
        size,
    );
    assert!(placed.changed);
    let placement = &state.render_placements(&grid)[0];
    assert_eq!(placement.display_cols, Some(5));
    assert_eq!(placement.occupied_cols, 5);
    assert!(placement_contains(&state.placements[0], 1, 10));

    let point_delete = state.apply(command("a=d,d=q,x=11,y=2,z=7,q=1", &[]), &mut grid, size);
    assert!(point_delete.changed);
    assert!(state.placements.is_empty());

    state.apply(
        command("a=p,i=85,p=2,c=5,r=2,C=1,q=1", &[]),
        &mut grid,
        size,
    );
    let column_delete = state.apply(command("a=d,d=x,x=11,q=1", &[]), &mut grid, size);
    assert!(column_delete.changed);
    assert!(state.placements.is_empty());

    state.apply(command("a=p,i=85,p=3,r=3,C=1,q=1", &[]), &mut grid, size);
    let row_sized = &state.render_placements(&grid)[0];
    assert_eq!(row_sized.display_cols, Some(12));
    assert_eq!(row_sized.occupied_cols, 12);
}

#[test]
fn natural_size_placement_still_truncates_at_the_right_edge() {
    let size = Size {
        cols: 8,
        rows: 4,
        cell_width: 1.0,
        cell_height: 1.0,
    };
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(size.cols, size.rows, 0, CursorStyle::Block);
    state.apply(
        command("a=t,f=32,s=4,v=1,i=86,q=1", &[255; 16]),
        &mut grid,
        size,
    );
    grid.set_cursor_position(1, 6);

    let placed = state.apply(command("a=p,i=86,p=1,C=1,q=1", &[]), &mut grid, size);

    assert!(placed.changed);
    let placement = &state.render_placements(&grid)[0];
    assert_eq!(placement.source_width, 2);
    assert_eq!(placement.display_cols, Some(2));
    assert_eq!(placement.occupied_cols, 2);
    assert!(!placement_contains(&state.placements[0], 1, 8));
}

#[test]
fn rejects_truncated_and_crc_corrupted_png_payloads() {
    let png = encode_png(1, 1, 4, &[255, 0, 0, 255]);
    for invalid in [png[..png.len() - 5].to_vec(), {
        let mut corrupted = png.clone();
        corrupted[29] ^= 1;
        corrupted
    }] {
        let payload = encode_base64(&invalid);
        let mut state = GraphicsState::default();
        let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);
        let result = state.apply(
            GraphicsCommand::parse(format!("a=T,f=100,i=9;{payload}").into_bytes(), false),
            &mut grid,
            Size::default(),
        );
        assert!(!result.changed);
        assert_eq!(result.replies, b"\x1b_Gi=9;EINVAL:invalid PNG image\x1b\\");
        assert!(state.render_placements(&grid).is_empty());
    }
}

#[test]
fn rejects_crc_valid_corrupt_idat_and_wrong_inflated_lengths() {
    let corrupt = png_document(1, 1, 8, 6, 0, vec![(*b"IDAT", vec![0x78, 0x01, 0xff])]);
    assert!(validate_png(&corrupt).is_err());

    let short = png_with_filtered_data(1, 1, 8, 6, 0, &[0, 0, 0, 0]);
    let long = png_with_filtered_data(1, 1, 8, 6, 0, &[0, 0, 0, 0, 0, 0]);
    assert!(validate_png(&short).is_err());
    assert!(validate_png(&long).is_err());
}

#[test]
fn rejects_transparency_chunk_for_rgba_without_storing_or_placing_it() {
    let png = png_document(
        1,
        1,
        8,
        6,
        0,
        vec![
            (*b"tRNS", vec![0]),
            (*b"IDAT", zlib_store(&[0, 0, 0, 0, 0])),
        ],
    );
    let payload = encode_base64(&png);
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);

    let result = state.apply(
        GraphicsCommand::parse(format!("a=T,f=100,i=9;{payload}").into_bytes(), false),
        &mut grid,
        Size::default(),
    );

    assert!(!result.changed);
    assert_eq!(result.replies, b"\x1b_Gi=9;EINVAL:invalid PNG image\x1b\\");
    assert!(!state.images.contains_key(&9));
    assert!(state.render_placements(&grid).is_empty());
}

#[test]
fn accepts_well_formed_transparency_for_supported_png_color_types() {
    let grayscale = png_document(
        1,
        1,
        8,
        0,
        0,
        vec![
            (*b"tRNS", 128u16.to_be_bytes().to_vec()),
            (*b"IDAT", zlib_store(&[0, 128])),
        ],
    );
    assert_eq!(validate_png(&grayscale).unwrap(), (1, 1));

    let indexed = png_document(
        1,
        1,
        1,
        3,
        0,
        vec![
            (*b"PLTE", vec![1, 2, 3]),
            (*b"tRNS", vec![0]),
            (*b"IDAT", zlib_store(&[0, 0])),
        ],
    );
    assert_eq!(validate_png(&indexed).unwrap(), (1, 1));
}

#[test]
fn rejects_invalid_png_scanline_filter() {
    let png = png_with_filtered_data(1, 1, 8, 6, 0, &[5, 0, 0, 0, 0]);
    assert!(validate_png(&png).is_err());
}

#[test]
fn accepts_consecutive_split_idat_but_rejects_interrupted_idat() {
    let compressed = zlib_store(&[0, 1, 2, 3, 4]);
    let split = compressed.len() / 2;
    let first = compressed[..split].to_vec();
    let second = compressed[split..].to_vec();
    let consecutive = png_document(
        1,
        1,
        8,
        6,
        0,
        vec![(*b"IDAT", first.clone()), (*b"IDAT", second.clone())],
    );
    assert_eq!(validate_png(&consecutive).unwrap(), (1, 1));

    let interrupted = png_document(
        1,
        1,
        8,
        6,
        0,
        vec![
            (*b"IDAT", first),
            (*b"tEXt", b"metadata".to_vec()),
            (*b"IDAT", second),
        ],
    );
    assert!(validate_png(&interrupted).is_err());
}

#[test]
fn png_idat_data_borrows_one_chunk_and_joins_split_chunks() {
    let compressed = zlib_store(&[0, 1, 2, 3, 4]);
    let single = png_document(1, 1, 8, 6, 0, vec![(*b"IDAT", compressed.clone())]);
    let single_data = png_idat_data(&single, compressed.len()).unwrap();
    assert!(matches!(single_data, Cow::Borrowed(_)));
    assert_eq!(single_data.as_ref(), compressed);

    let split = compressed.len() / 2;
    let multiple = png_document(
        1,
        1,
        8,
        6,
        0,
        vec![
            (*b"IDAT", compressed[..split].to_vec()),
            (*b"IDAT", compressed[split..].to_vec()),
        ],
    );
    let multiple_data = png_idat_data(&multiple, compressed.len()).unwrap();
    assert!(matches!(multiple_data, Cow::Owned(_)));
    assert_eq!(multiple_data.as_ref(), compressed);
}

#[test]
fn enforces_indexed_palette_rules() {
    let compressed = zlib_store(&[0, 0]);
    let missing = png_document(1, 1, 1, 3, 0, vec![(*b"IDAT", compressed.clone())]);
    assert!(validate_png(&missing).is_err());

    let valid = png_document(
        1,
        1,
        1,
        3,
        0,
        vec![(*b"PLTE", vec![0, 0, 0]), (*b"IDAT", compressed.clone())],
    );
    assert_eq!(validate_png(&valid).unwrap(), (1, 1));

    let too_many_entries = png_document(
        1,
        1,
        1,
        3,
        0,
        vec![(*b"PLTE", vec![0; 9]), (*b"IDAT", compressed)],
    );
    assert!(validate_png(&too_many_entries).is_err());

    for color_type in [0, 4] {
        let filtered = if color_type == 0 {
            vec![0, 0]
        } else {
            vec![0, 0, 0]
        };
        let forbidden = png_document(
            1,
            1,
            8,
            color_type,
            0,
            vec![(*b"PLTE", vec![0, 0, 0]), (*b"IDAT", zlib_store(&filtered))],
        );
        assert!(validate_png(&forbidden).is_err());
    }
}

#[test]
fn rejects_unknown_critical_chunks_but_allows_ancillary_chunks() {
    let compressed = zlib_store(&[0, 0, 0, 0, 0]);
    let critical = png_document(
        1,
        1,
        8,
        6,
        0,
        vec![(*b"ABCD", Vec::new()), (*b"IDAT", compressed.clone())],
    );
    assert!(validate_png(&critical).is_err());

    let ancillary = png_document(
        1,
        1,
        8,
        6,
        0,
        vec![(*b"abCd", b"metadata".to_vec()), (*b"IDAT", compressed)],
    );
    assert_eq!(validate_png(&ancillary).unwrap(), (1, 1));
}

#[test]
fn sanitizes_compressed_metadata_and_apng_without_changing_static_pixels() {
    let idat = zlib_store(&[0, 10, 20, 30, 255]);
    let safe_text = b"Author\0Termy".to_vec();
    let safe_itxt = b"Description\0\x00\x00en\0Title\0safe text containing 1".to_vec();
    let mut compressed_itxt = b"Comment\0\x01\x00\0\0".to_vec();
    compressed_itxt.extend_from_slice(&zlib_store(b"expanded comment"));
    assert!(!png_itxt_is_compressed(&safe_itxt).unwrap());
    assert!(png_itxt_is_compressed(&compressed_itxt).unwrap());

    let mut iccp = b"profile\0\0".to_vec();
    iccp.extend_from_slice(&zlib_store(b"expanded color profile"));
    let mut ztxt = b"Comment\0\0".to_vec();
    ztxt.extend_from_slice(&zlib_store(b"expanded text"));
    let frame_control = |sequence_number: u32| {
        let mut chunk = Vec::with_capacity(26);
        chunk.extend_from_slice(&sequence_number.to_be_bytes());
        chunk.extend_from_slice(&1u32.to_be_bytes());
        chunk.extend_from_slice(&1u32.to_be_bytes());
        chunk.extend_from_slice(&0u32.to_be_bytes());
        chunk.extend_from_slice(&0u32.to_be_bytes());
        chunk.extend_from_slice(&1u16.to_be_bytes());
        chunk.extend_from_slice(&100u16.to_be_bytes());
        chunk.extend_from_slice(&[0, 0]);
        chunk
    };
    let mut frame_data = 2u32.to_be_bytes().to_vec();
    frame_data.extend_from_slice(&zlib_store(&[0, 40, 50, 60, 255]));
    let png = png_document(
        1,
        1,
        8,
        6,
        0,
        vec![
            (*b"tEXt", safe_text.clone()),
            (*b"iTXt", safe_itxt.clone()),
            (*b"iCCP", iccp),
            (*b"zTXt", ztxt),
            (*b"iTXt", compressed_itxt),
            (*b"acTL", [2u32.to_be_bytes(), 0u32.to_be_bytes()].concat()),
            (*b"fcTL", frame_control(0)),
            (*b"IDAT", idat.clone()),
            (*b"fcTL", frame_control(1)),
            (*b"fdAT", frame_data),
        ],
    );
    assert_eq!(validate_png(&png).unwrap(), (1, 1));

    let original_len = png.len();
    let original_pointer = png.as_ptr();
    let original_capacity = png.capacity();
    let (sanitized, width, height) = sanitize_png(png).unwrap();

    assert_eq!((width, height), (1, 1));
    assert!(sanitized.len() < original_len);
    assert_eq!(sanitized.as_ptr(), original_pointer);
    assert_eq!(sanitized.capacity(), original_capacity);
    assert_eq!(validate_png(&sanitized).unwrap(), (1, 1));
    let chunks = png_chunks(&sanitized);
    assert!(
        chunks
            .iter()
            .all(|(kind, _)| !matches!(kind, b"iCCP" | b"zTXt" | b"acTL" | b"fcTL" | b"fdAT"))
    );
    assert_eq!(
        chunks
            .iter()
            .filter(|(kind, _)| kind == b"iTXt")
            .map(|(_, payload)| payload.as_slice())
            .collect::<Vec<_>>(),
        vec![safe_itxt.as_slice()]
    );
    assert!(chunks.contains(&(*b"tEXt", safe_text)));
    assert!(chunks.contains(&(*b"IDAT", idat)));
}

#[test]
fn rejects_malformed_itxt_instead_of_forwarding_it() {
    let invalid_payloads = [
        b"missing terminator".as_slice(),
        b"key\0\x02\x00\0\0".as_slice(),
        b"key\0\x00\x01\0\0".as_slice(),
        b"key\0\x00\x00missing language terminator".as_slice(),
        b"key\0\x00\x00en\0missing translated terminator".as_slice(),
    ];
    for payload in invalid_payloads {
        let png = png_document(
            1,
            1,
            8,
            6,
            0,
            vec![
                (*b"iTXt", payload.to_vec()),
                (*b"IDAT", zlib_store(&[0, 0, 0, 0, 0])),
            ],
        );
        assert!(sanitize_png(png).is_err());
    }
}

#[test]
fn rejects_invalid_chunk_names_and_reserved_bits() {
    let compressed = zlib_store(&[0, 0, 0, 0, 0]);
    for kind in [*b"abcd", *b"a1Cd"] {
        let png = png_document(
            1,
            1,
            8,
            6,
            0,
            vec![(kind, Vec::new()), (*b"IDAT", compressed.clone())],
        );
        assert!(validate_png(&png).is_err());
    }
}

#[test]
fn validates_adam7_edge_dimensions_and_pass_layout() {
    let one_pixel = png_with_filtered_data(1, 1, 8, 6, 1, &[0, 0, 0, 0, 0]);
    assert_eq!(validate_png(&one_pixel).unwrap(), (1, 1));

    // A 5x5 RGBA8 Adam7 image has 11 scanlines across all seven passes:
    // 100 pixel bytes plus 11 filter bytes.
    let filtered = vec![0; 111];
    let complete = png_with_filtered_data(5, 5, 8, 6, 1, &filtered);
    assert_eq!(validate_png(&complete).unwrap(), (5, 5));

    let short = png_with_filtered_data(5, 5, 8, 6, 1, &filtered[..110]);
    assert!(validate_png(&short).is_err());

    let mut invalid_late_filter = filtered;
    invalid_late_filter[90] = 5;
    let invalid = png_with_filtered_data(5, 5, 8, 6, 1, &invalid_late_filter);
    assert!(validate_png(&invalid).is_err());
}

#[test]
fn assembles_chunked_and_all_zlib_block_type_uploads() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);
    let pixels = [12, 34, 56, 255];

    let first = state.apply(
        command("a=T,f=32,s=1,v=1,i=20,m=1,q=1", &pixels[..2]),
        &mut grid,
        test_size(),
    );
    assert!(!first.changed);
    let second = state.apply(command("m=0,q=1", &pixels[2..]), &mut grid, test_size());
    assert!(second.changed);

    let compressed = zlib_store(&pixels);
    let compressed_result = state.apply(
        command("a=T,f=32,s=1,v=1,o=z,i=21,C=1,q=1", &compressed),
        &mut grid,
        test_size(),
    );
    assert!(compressed_result.changed);

    let fixed = decode_hex("7801e351b2f80f0002090166");
    let fixed_result = state.apply(
        command("a=T,f=32,s=1,v=1,o=z,i=22,C=1,q=1", &fixed),
        &mut grid,
        test_size(),
    );
    assert!(fixed_result.changed);

    let dynamic = decode_hex(
        "78daedccc51180300000c13fd5e0524ed024b84bf56815cc6d012bc427fe24aff4953df25b7191522aa5b42ecbaaaa9bb6eb87719a9775db0fd3b21dd7f383303204252525252525252525252525252525252525252525252525252525252525e57fcb1333683551",
    );
    let dynamic_result = state.apply(
        command("a=T,f=32,s=25,v=83,o=z,i=23,C=1,q=1", &dynamic),
        &mut grid,
        test_size(),
    );
    assert!(dynamic_result.changed);
    assert_eq!(state.render_placements(&grid).len(), 4);
}

#[test]
fn temporary_file_transfer_removes_protocol_file_after_bounded_read() {
    let temp = TestTempPath::new();
    let path = temp.file("tty-graphics-protocol-success", b"headerpixelsfooter");

    let data = resolve_transmission_data(&command("t=t,O=6,S=6", &[]), path_bytes(&path)).unwrap();

    assert_eq!(data, b"pixels");
    assert!(!path.exists());
}

#[test]
fn temporary_file_transfer_preserves_protocol_file_when_read_fails() {
    let temp = TestTempPath::new();
    let path = temp.file("tty-graphics-protocol-read-error", b"x");

    let result = resolve_transmission_data(&command("t=t,O=2", &[]), path_bytes(&path));

    assert!(result.is_err());
    assert!(path.exists());
}

#[test]
fn regular_file_transfer_never_removes_source_file() {
    let temp = TestTempPath::new();
    let path = temp.file("tty-graphics-protocol-regular", b"pixels");

    let data = resolve_transmission_data(&command("t=f", &[]), path_bytes(&path)).unwrap();

    assert_eq!(data, b"pixels");
    assert!(path.exists());
}

#[cfg(unix)]
#[test]
fn file_transfer_rejects_fifo_without_waiting_for_a_writer() {
    use std::{fs::OpenOptions, sync::mpsc, thread, time::Duration};

    let temp = TestTempPath::new();
    let fifo = temp.path.join("image-fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );

    let worker_path = fifo.clone();
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = sender.send(read_regular_file(&worker_path, 0, None));
    });

    match receiver.recv_timeout(Duration::from_millis(500)) {
        Ok(result) => assert_eq!(result, Err("EINVAL:invalid image file".into())),
        Err(error) => {
            // Keep the regression test itself bounded if the open-before-check
            // bug returns: a writer releases the blocked reader before failure.
            let _ = OpenOptions::new().write(true).open(&fifo);
            let _ = receiver.recv_timeout(Duration::from_secs(1));
            worker.join().unwrap();
            panic!("FIFO image read did not return promptly: {error}");
        }
    }
    worker.join().unwrap();
}

#[test]
fn temporary_file_transfer_preserves_non_protocol_file() {
    let temp = TestTempPath::new();
    let path = temp.file("ordinary-image-data", b"pixels");

    let data = resolve_transmission_data(&command("t=t", &[]), path_bytes(&path)).unwrap();

    assert_eq!(data, b"pixels");
    assert!(path.exists());
}

#[test]
fn temporary_file_transfer_does_not_trust_a_removed_parent_component() {
    let temp = TestTempPath::new();
    let path = temp.file("ordinary-image-data", b"pixels");
    let marker = temp.path.join("tty-graphics-protocol-decoy");
    std::fs::create_dir(&marker).unwrap();
    let disguised = marker.join("..").join("ordinary-image-data");

    let data = resolve_transmission_data(&command("t=t", &[]), path_bytes(&disguised)).unwrap();

    assert_eq!(data, b"pixels");
    assert!(path.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn temporary_file_transfer_removes_protocol_file_from_dev_shm() {
    let root = PathBuf::from("/dev/shm");
    if !root.is_dir() {
        return;
    }
    let path = root.join(format!(
        "tty-graphics-protocol-termy-test-{}-{}",
        std::process::id(),
        TestTempPath::new()
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
    ));
    std::fs::write(&path, b"pixels").unwrap();

    let data = resolve_transmission_data(&command("t=t", &[]), path_bytes(&path)).unwrap();

    assert_eq!(data, b"pixels");
    assert!(!path.exists());
}

#[test]
fn delete_selectors_target_coordinates_rows_columns_z_and_ids() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);
    upload_one_pixel(&mut state, &mut grid, 30);

    for (row, col, placement_id, z_index) in [(0, 0, 1, 7), (0, 4, 2, 8), (4, 0, 3, 7)] {
        grid.set_cursor_position(row, col);
        let result = state.apply(
            command(
                &format!("a=p,i=30,p={placement_id},c=2,r=2,z={z_index},C=1,q=1"),
                &[],
            ),
            &mut grid,
            test_size(),
        );
        assert!(result.changed);
    }

    state.apply(
        command("a=d,d=q,x=1,y=1,z=7,q=1", &[]),
        &mut grid,
        test_size(),
    );
    let placements = state.render_placements(&grid);
    assert_eq!(placements.len(), 2);
    assert!(
        placements
            .iter()
            .all(|placement| placement.placement_id != 1)
    );

    state.apply(command("a=d,d=x,x=1,q=1", &[]), &mut grid, test_size());
    let placements = state.render_placements(&grid);
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].placement_id, 2);

    state.apply(command("a=d,d=i,i=30,p=2,q=1", &[]), &mut grid, test_size());
    assert!(state.render_placements(&grid).is_empty());
    assert!(
        state.images.contains_key(&30),
        "lowercase delete keeps image data"
    );

    state.apply(command("a=d,d=I,i=30,q=1", &[]), &mut grid, test_size());
    assert!(!state.images.contains_key(&30));
}

#[test]
fn placement_count_is_bounded_and_evicts_the_oldest() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);
    upload_one_pixel(&mut state, &mut grid, 40);
    for index in 0..=MAX_PLACEMENTS {
        grid.set_cursor_position(index % 24, index % 80);
        state.apply(
            command("a=p,i=40,c=1,r=1,C=1,q=1", &[]),
            &mut grid,
            test_size(),
        );
    }
    assert_eq!(state.placements.len(), MAX_PLACEMENTS);
    assert_eq!(state.placements.first().unwrap().serial, 2);
    assert_eq!(
        state.placements.last().unwrap().serial,
        (MAX_PLACEMENTS + 1) as u64
    );
}
