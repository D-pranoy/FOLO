//! Exact bytes of DDP and DNRGB(W) packets: the golden vectors, the 1440-byte split, offsets, the
//! push flag, the sequence wrap and the timeout byte, checked also by the fake receiver that
//! parses them as WLED does.

use folo_provider_wled::ddp::{DdpEncoder, PixelFormat, PixelRun};
use folo_provider_wled::dnrgb::{DnrgbEncoder, MAX_TIMEOUT_S};
use folo_provider_wled::testing::{DdpFrames, DdpPacket, DnrgbPacket};

fn ramp(len: usize) -> Vec<u8> {
    (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

fn run(format: PixelFormat, first_pixel: u32, data: &[u8]) -> PixelRun<'_> {
    PixelRun {
        format,
        first_pixel,
        data,
    }
}

#[test]
fn one_rgb_packet_is_ten_bytes_of_header_and_the_pixels() {
    let data = [255, 0, 0, 0, 255, 0];
    let packets = DdpEncoder::new().frame(&[run(PixelFormat::Rgb, 0, &data)]);
    // flags 0x40|push, sequence 1, RGB type, destination 1, offset 0, length 6
    let golden: Vec<u8> = vec![
        0x41, 0x01, 0x0B, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 255, 0, 0, 0, 255, 0,
    ];
    assert_eq!(packets, vec![golden]);
}

#[test]
fn rgbw_pixels_use_type_1b_and_the_offset_counts_bytes() {
    let data = [1, 2, 3, 4];
    let packets = DdpEncoder::new().frame(&[run(PixelFormat::Rgbw, 10, &data)]);
    let golden: Vec<u8> = vec![
        0x41, 0x01, 0x1B, 0x01, 0x00, 0x00, 0x00, 0x28, 0x00, 0x04, 1, 2, 3, 4,
    ];
    assert_eq!(packets, vec![golden]);
}

#[test]
fn rgb_offsets_are_three_bytes_a_pixel() {
    let packets = DdpEncoder::new().frame(&[run(PixelFormat::Rgb, 300, &[9, 9, 9])]);
    // 300 * 3 = 900 = 0x0384
    assert_eq!(&packets[0][4..8], &[0x00, 0x00, 0x03, 0x84]);
}

#[test]
fn a_frame_splits_at_1440_data_bytes_and_only_the_last_packet_pushes() {
    // 481 RGB pixels are 1443 bytes
    let data = ramp(481 * 3);
    let packets = DdpEncoder::new().frame(&[run(PixelFormat::Rgb, 0, &data)]);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].len(), 10 + 1440);
    assert_eq!(packets[1].len(), 10 + 3);
    // Header of the first: no push, sequence 1, offset 0, length 0x05A0
    assert_eq!(
        &packets[0][..10],
        &[0x40, 0x01, 0x0B, 0x01, 0, 0, 0, 0, 0x05, 0xA0]
    );
    // Header of the second: push, sequence 2, offset 1440 = 0x05A0, length 3
    assert_eq!(
        &packets[1][..10],
        &[0x41, 0x02, 0x0B, 0x01, 0, 0, 0x05, 0xA0, 0x00, 0x03]
    );
    assert_eq!(&packets[0][10..], &data[..1440]);
    assert_eq!(&packets[1][10..], &data[1440..]);
}

#[test]
fn an_exact_multiple_of_1440_bytes_has_no_empty_packet() {
    let data = ramp(480 * 3);
    let packets = DdpEncoder::new().frame(&[run(PixelFormat::Rgb, 0, &data)]);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0][0], 0x41);
}

#[test]
fn rgbw_splits_at_360_pixels_and_keeps_the_pixel_offset() {
    let data = ramp(361 * 4);
    let packets = DdpEncoder::new().frame(&[run(PixelFormat::Rgbw, 5, &data)]);
    assert_eq!(packets.len(), 2);
    // 5 * 4 = 20, then 20 + 1440
    assert_eq!(&packets[0][4..8], &20u32.to_be_bytes());
    assert_eq!(&packets[1][4..8], &1460u32.to_be_bytes());
    assert_eq!(&packets[1][8..10], &4u16.to_be_bytes());
}

#[test]
fn several_runs_share_one_push_on_the_very_last_packet() {
    let a = ramp(6);
    let b = ramp(8);
    let packets =
        DdpEncoder::new().frame(&[run(PixelFormat::Rgb, 0, &a), run(PixelFormat::Rgbw, 20, &b)]);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0][0], 0x40, "no push on the first run's packet");
    assert_eq!(packets[1][0], 0x41);
    assert_eq!(packets[1][2], 0x1B);
}

#[test]
fn the_sequence_counts_packets_from_1_to_15_and_wraps_to_1() {
    let mut encoder = DdpEncoder::new();
    let mut sequences = Vec::new();
    for _ in 0..17 {
        let packets = encoder.frame(&[run(PixelFormat::Rgb, 0, &[1, 2, 3])]);
        sequences.push(packets[0][1]);
    }
    let expected: Vec<u8> = (1..=15).chain([1, 2]).collect();
    assert_eq!(sequences, expected);
}

#[test]
fn a_frame_with_nothing_in_it_has_no_packets() {
    assert_eq!(DdpEncoder::new().frame(&[]).len(), 0);
    assert_eq!(
        DdpEncoder::new()
            .frame(&[run(PixelFormat::Rgb, 0, &[])])
            .len(),
        0
    );
}

#[test]
fn bytes_that_are_not_a_whole_pixel_are_dropped() {
    let packets = DdpEncoder::new().frame(&[run(PixelFormat::Rgb, 0, &[1, 2, 3, 4, 5])]);
    assert_eq!(&packets[0][8..10], &3u16.to_be_bytes());
}

#[test]
fn the_receiver_renders_a_frame_only_on_push() {
    let data = ramp(481 * 3);
    let packets = DdpEncoder::new().frame(&[run(PixelFormat::Rgb, 0, &data)]);
    let mut frames = DdpFrames::default();
    let first = DdpPacket::parse(&packets[0]).unwrap();
    assert!(!first.push());
    assert_eq!(first.channels(), 3);
    frames.apply(&first);
    assert_eq!(frames.rendered.len(), 0, "nothing is shown before the push");
    frames.apply(&DdpPacket::parse(&packets[1]).unwrap());
    assert_eq!(frames.rendered.len(), 1);
    assert_eq!(frames.rendered[0], data);
}

#[test]
fn the_receiver_reads_rgbw_packets_as_four_channels() {
    let packets = DdpEncoder::new().frame(&[run(PixelFormat::Rgbw, 0, &[1, 2, 3, 4])]);
    assert_eq!(DdpPacket::parse(&packets[0]).unwrap().channels(), 4);
}

#[test]
fn the_receiver_refuses_what_wled_refuses() {
    assert!(DdpPacket::parse(&[0x41; 4]).is_err());
    assert!(DdpPacket::parse(&[0x00, 1, 0x0B, 1, 0, 0, 0, 0, 0, 0]).is_err());
    // Length says 6, only 3 follow
    assert!(DdpPacket::parse(&[0x41, 1, 0x0B, 1, 0, 0, 0, 0, 0, 6, 1, 2, 3]).is_err());
}

#[test]
fn dnrgb_golden_bytes() {
    let packets = DnrgbEncoder::new(2).pixels(PixelFormat::Rgb, 258, &[1, 2, 3, 4, 5, 6]);
    // protocol 4, timeout 2 s, start index 258 = 0x0102
    assert_eq!(packets, vec![vec![4, 2, 0x01, 0x02, 1, 2, 3, 4, 5, 6]]);
}

#[test]
fn dnrgbw_golden_bytes() {
    let packets = DnrgbEncoder::new(2).pixels(PixelFormat::Rgbw, 0, &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(packets, vec![vec![5, 2, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]]);
}

#[test]
fn dnrgb_splits_at_489_pixels_and_dnrgbw_at_367() {
    let rgb = ramp(500 * 3);
    let packets = DnrgbEncoder::new(2).pixels(PixelFormat::Rgb, 0, &rgb);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].len(), 4 + 489 * 3);
    assert_eq!(&packets[1][2..4], &489u16.to_be_bytes());
    assert_eq!(packets[1].len(), 4 + 11 * 3);

    let rgbw = ramp(400 * 4);
    let packets = DnrgbEncoder::new(2).pixels(PixelFormat::Rgbw, 10, &rgbw);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].len(), 4 + 367 * 4);
    assert_eq!(&packets[1][2..4], &377u16.to_be_bytes());
}

#[test]
fn a_lone_trailing_pixel_is_paired_with_the_one_before() {
    // 490 pixels would leave one pixel alone in the second packet, which WLED ignores
    let data = ramp(490 * 3);
    let packets = DnrgbEncoder::new(2).pixels(PixelFormat::Rgb, 0, &data);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].len(), 4 + 488 * 3);
    assert_eq!(&packets[1][2..4], &488u16.to_be_bytes());
    assert_eq!(packets[1].len(), 4 + 2 * 3);
    for packet in &packets {
        assert!(DnrgbPacket::parse(packet).is_ok());
    }
}

#[test]
fn a_single_pixel_has_no_dnrgb_packet() {
    // WLED ignores a packet of one pixel, and a copy on the next LED would overwrite it
    let packets = DnrgbEncoder::new(2).pixels(PixelFormat::Rgb, 7, &[9, 8, 7]);
    assert_eq!(packets.len(), 0);
}

#[test]
fn pixels_beyond_the_16_bit_index_are_dropped() {
    let packets = DnrgbEncoder::new(2).pixels(PixelFormat::Rgb, 65_534, &ramp(6 * 3));
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].len(), 4 + 2 * 3);
}

#[test]
fn the_timeout_byte_is_kept_clear_of_forever_and_of_zero() {
    assert_eq!(DnrgbEncoder::new(255).timeout_s(), MAX_TIMEOUT_S);
    assert_eq!(DnrgbEncoder::new(0).timeout_s(), 1);
    assert_eq!(DnrgbEncoder::new(5).timeout_s(), 5);
}

#[test]
fn the_release_packet_has_timeout_zero() {
    let release = DnrgbEncoder::new(7).release();
    assert_eq!(release, vec![4, 0, 0, 0]);
    assert!(DnrgbPacket::parse(&release).unwrap().releases());
}
