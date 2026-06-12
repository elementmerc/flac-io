// Ogg encapsulation for FLAC (the `.oga` form).
//
// FLAC can travel two ways. The native `.flac` stream is `fLaC` then metadata
// blocks then audio frames, back to back. The Ogg form wraps that same FLAC
// data inside the generic Ogg container (the one Vorbis and Opus also use):
// the FLAC bytes are cut into "packets", packets are grouped into "pages", and
// every page carries a header with a magic `OggS`, a granule (sample) position,
// a stream serial number, a sequence number, a checksum, and a table that says
// where each packet begins and ends.
//
// The FLAC inside is unchanged. Each audio frame is exactly one Ogg packet and
// each metadata block is one packet, so decoding is "demux the pages back into
// the original FLAC byte stream, then hand it to the native decoder". Encoding
// is the reverse: take the native FLAC pieces and page them up.
//
// This module is a parser of untrusted bytes, so every field is bounds-checked,
// every page checksum is verified, and no input can make it panic.

use crate::crc::ogg_crc32;
use crate::error::FlacError;
use crate::metadata::FLAC_MARKER;

/// The four-byte Ogg page capture pattern.
const OGG_MAGIC: &[u8; 4] = b"OggS";
/// Fixed part of a page header, before the variable segment table.
const OGG_HEADER_LEN: usize = 27;
/// First byte of the FLAC-to-Ogg mapping header packet, then the signature.
const FLAC_MAPPING_TYPE: u8 = 0x7F;
const FLAC_MAPPING_SIG: &[u8; 4] = b"FLAC";

/// Header-type flag bits in byte 5 of a page header.
const FLAG_CONTINUED: u8 = 0x01;
const FLAG_BOS: u8 = 0x02;
const FLAG_EOS: u8 = 0x04;

/// Length of the mapping-header prefix that precedes the embedded `fLaC`
/// signature: type byte (1) + "FLAC" (4) + major/minor version (2) + the
/// 16-bit header-packet count (2).
const MAPPING_PREFIX_LEN: usize = 9;

/// Serial number the encoder stamps on the single logical stream it writes. A
/// fixed value keeps the output byte-stable (Ogg only needs the serial to be
/// unique among multiplexed streams, and the encoder writes exactly one).
const ENCODER_SERIAL: u32 = 0x664C_4143; // "fLAC" as bytes, purely a label

/// True when the bytes look like an Ogg stream (used to choose the decode path).
pub(crate) fn is_ogg(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[0..4] == OGG_MAGIC
}

/// Rebuild the native FLAC byte stream carried inside an Ogg stream.
///
/// With `headers_only` the rebuild stops once every metadata block has been
/// recovered, so `info` does not have to demux a whole file. The returned bytes
/// are a valid native FLAC stream that the ordinary decoder can read.
pub(crate) fn to_native_flac(bytes: &[u8], headers_only: bool) -> Result<Vec<u8>, FlacError> {
    let mut pos = 0usize;
    let mut serial: Option<u32> = None;
    let mut partial: Vec<u8> = Vec::new();
    let mut packets: Vec<Vec<u8>> = Vec::new();
    // Number of header packets to keep for `headers_only`: the mapping header
    // plus the count it declares. Filled in once the first packet is parsed.
    let mut wanted_headers: Option<usize> = None;

    while pos + OGG_HEADER_LEN <= bytes.len() {
        if &bytes[pos..pos + 4] != OGG_MAGIC {
            // Once our stream is found, anything that is not a page boundary is
            // a malformed continuation; before it is found it means the input
            // is not the Ogg stream we can read.
            return Err(FlacError::CorruptStream(
                "expected an Ogg page boundary".into(),
            ));
        }
        if bytes[pos + 4] != 0 {
            return Err(FlacError::Unsupported(format!(
                "Ogg stream structure version {}",
                bytes[pos + 4]
            )));
        }
        let flags = bytes[pos + 5];
        let page_serial = u32::from_le_bytes([
            bytes[pos + 14],
            bytes[pos + 15],
            bytes[pos + 16],
            bytes[pos + 17],
        ]);
        let n_segments = bytes[pos + 26] as usize;
        let seg_table = pos + OGG_HEADER_LEN;
        let body_start = seg_table + n_segments;
        if body_start > bytes.len() {
            return Err(FlacError::Truncated);
        }
        let body_len: usize = bytes[seg_table..body_start]
            .iter()
            .map(|&s| s as usize)
            .sum();
        let body_end = body_start
            .checked_add(body_len)
            .ok_or(FlacError::Truncated)?;
        if body_end > bytes.len() {
            return Err(FlacError::Truncated);
        }

        let is_flac_bos = flags & FLAG_BOS != 0
            && body_len >= 5
            && bytes[body_start] == FLAC_MAPPING_TYPE
            && &bytes[body_start + 1..body_start + 5] == FLAC_MAPPING_SIG;

        if serial.is_none() {
            // Hunt for the FLAC logical stream's start page; skip any other
            // (a multiplexed Ogg can carry several streams).
            if is_flac_bos {
                serial = Some(page_serial);
            } else {
                pos = body_end;
                continue;
            }
        } else if Some(page_serial) != serial {
            pos = body_end;
            continue;
        }

        // Verify the checksum only on pages of our stream (computed over the
        // page with its own checksum field zeroed). Checking only owned pages
        // means a damaged page in an unrelated multiplexed stream does not stop
        // us recovering the FLAC audio. A mismatch on our page means it is
        // damaged.
        let stored_crc = u32::from_le_bytes([
            bytes[pos + 22],
            bytes[pos + 23],
            bytes[pos + 24],
            bytes[pos + 25],
        ]);
        let mut page = bytes[pos..body_end].to_vec();
        page[22..26].fill(0);
        if ogg_crc32(&page) != stored_crc {
            return Err(FlacError::CrcMismatch);
        }

        // Reassemble packets from this page's segments. A packet is a run of
        // 255-valued segments terminated by a segment below 255; the run can
        // cross pages, which is why `partial` carries over.
        let mut seg_pos = body_start;
        for i in 0..n_segments {
            let s = bytes[seg_table + i] as usize;
            partial.extend_from_slice(&bytes[seg_pos..seg_pos + s]);
            seg_pos += s;
            if s < 255 {
                packets.push(std::mem::take(&mut partial));
                if wanted_headers.is_none() {
                    wanted_headers = Some(1 + mapping_header_count(&packets[0])?);
                }
                if headers_only {
                    if let Some(w) = wanted_headers {
                        if packets.len() >= w {
                            return rebuild(&packets, Some(w));
                        }
                    }
                }
            }
        }

        if flags & FLAG_EOS != 0 {
            break;
        }
        pos = body_end;
    }

    if serial.is_none() {
        return Err(FlacError::NotFlac);
    }
    let keep = if headers_only { wanted_headers } else { None };
    rebuild(&packets, keep)
}

/// Read the 16-bit "number of following header packets" field from the mapping
/// header packet, validating its signature.
fn mapping_header_count(first: &[u8]) -> Result<usize, FlacError> {
    if first.len() < MAPPING_PREFIX_LEN + 4
        || first[0] != FLAC_MAPPING_TYPE
        || &first[1..5] != FLAC_MAPPING_SIG
        || &first[MAPPING_PREFIX_LEN..MAPPING_PREFIX_LEN + 4] != FLAC_MARKER
    {
        return Err(FlacError::CorruptStream(
            "invalid FLAC-to-Ogg mapping header".into(),
        ));
    }
    Ok(u16::from_be_bytes([first[7], first[8]]) as usize)
}

/// Concatenate the demuxed packets back into a native FLAC stream: the mapping
/// header's embedded `fLaC` and STREAMINFO, then every following packet (the
/// remaining metadata blocks and the audio frames) exactly as they were.
fn rebuild(packets: &[Vec<u8>], keep: Option<usize>) -> Result<Vec<u8>, FlacError> {
    let first = packets.first().ok_or(FlacError::Truncated)?;
    // Revalidate the mapping header (also covers the non-`headers_only` path).
    mapping_header_count(first)?;

    let limit = keep.unwrap_or(packets.len()).min(packets.len());
    let mut native = Vec::new();
    native.extend_from_slice(&first[MAPPING_PREFIX_LEN..]);
    for packet in &packets[1..limit] {
        native.extend_from_slice(packet);
    }
    Ok(native)
}

/// One packet to be paged, with the granule (running sample count) that holds
/// once its data has been decoded. Metadata packets carry granule 0.
pub(crate) struct Packet {
    pub data: Vec<u8>,
    pub granule: i64,
}

/// Page a sequence of FLAC packets into an Ogg stream.
///
/// The first packet (the FLAC mapping header) is placed alone on the first
/// page, as the mapping requires; the rest are packed densely, up to 255
/// segments per page. The single logical stream is stamped with a fixed serial
/// so the output is byte-stable.
pub(crate) fn mux(packets: &[Packet]) -> Vec<u8> {
    // One lacing segment per (up to) 255 bytes of packet data. The segment that
    // terminates a packet carries that packet's granule; a 255-valued segment
    // means the packet spills onto the next page. `body` is every packet's data
    // back to back, sliced out page by page below.
    struct Seg {
        value: u8,
        ends_granule: Option<i64>,
    }
    let mut segs: Vec<Seg> = Vec::new();
    let mut body: Vec<u8> = Vec::new();
    let mut first_packet_last_seg = 0usize;
    for (i, packet) in packets.iter().enumerate() {
        body.extend_from_slice(&packet.data);
        let mut remaining = packet.data.len();
        loop {
            let value = remaining.min(255);
            remaining -= value;
            let ends = value < 255;
            segs.push(Seg {
                value: value as u8,
                ends_granule: if ends { Some(packet.granule) } else { None },
            });
            if ends {
                break;
            }
        }
        if i == 0 {
            first_packet_last_seg = segs.len() - 1;
        }
    }

    let mut out = Vec::new();
    let mut seq: u32 = 0;
    let mut seg_idx = 0usize;
    let mut body_pos = 0usize;
    let mut first_page = true;
    let mut continued = false;

    while seg_idx < segs.len() {
        // Fill a page with up to 255 segments, but break right after the first
        // packet so the mapping header sits alone on the opening page.
        let mut count = 0usize;
        let mut page_granule: i64 = -1;
        while seg_idx + count < segs.len() && count < 255 {
            if let Some(g) = segs[seg_idx + count].ends_granule {
                page_granule = g;
            }
            count += 1;
            if seg_idx + count - 1 == first_packet_last_seg {
                break;
            }
        }

        let page_body_len: usize = segs[seg_idx..seg_idx + count]
            .iter()
            .map(|s| s.value as usize)
            .sum();
        let this_continued = continued;
        continued = segs[seg_idx + count - 1].value == 255;
        let is_eos = seg_idx + count >= segs.len();

        let mut flags = 0u8;
        if first_page {
            flags |= FLAG_BOS;
        }
        if is_eos {
            flags |= FLAG_EOS;
        }
        if this_continued {
            flags |= FLAG_CONTINUED;
        }

        let page_start = out.len();
        out.extend_from_slice(OGG_MAGIC);
        out.push(0); // stream structure version
        out.push(flags);
        out.extend_from_slice(&page_granule.to_le_bytes());
        out.extend_from_slice(&ENCODER_SERIAL.to_le_bytes());
        out.extend_from_slice(&seq.to_le_bytes());
        let crc_at = out.len();
        out.extend_from_slice(&[0u8; 4]); // checksum, patched in below
        out.push(count as u8);
        for j in 0..count {
            out.push(segs[seg_idx + j].value);
        }
        out.extend_from_slice(&body[body_pos..body_pos + page_body_len]);
        let crc = ogg_crc32(&out[page_start..]);
        out[crc_at..crc_at + 4].copy_from_slice(&crc.to_le_bytes());

        body_pos += page_body_len;
        seg_idx += count;
        seq += 1;
        first_page = false;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid-looking mapping header packet: the prefix, a header-packet count,
    /// then `fLaC` and `body` standing in for STREAMINFO (the Ogg layer only
    /// moves these bytes, it does not interpret them).
    fn mapping_header(count: u16, body: &[u8]) -> Vec<u8> {
        let mut p = vec![FLAC_MAPPING_TYPE];
        p.extend_from_slice(FLAC_MAPPING_SIG);
        p.extend_from_slice(&[1, 0]);
        p.extend_from_slice(&count.to_be_bytes());
        p.extend_from_slice(FLAC_MARKER);
        p.extend_from_slice(body);
        p
    }

    /// Expected native rebuild: the mapping header minus its prefix, then every
    /// following packet verbatim.
    fn expected_native(packets: &[Packet]) -> Vec<u8> {
        let mut v = packets[0].data[MAPPING_PREFIX_LEN..].to_vec();
        for p in &packets[1..] {
            v.extend_from_slice(&p.data);
        }
        v
    }

    #[test]
    fn is_ogg_detects_capture_pattern() {
        assert!(is_ogg(b"OggS....."));
        assert!(!is_ogg(b"fLaC"));
        assert!(!is_ogg(b"Og"));
    }

    #[test]
    fn mux_then_demux_round_trips() {
        let packets = vec![
            Packet {
                data: mapping_header(0, &[0xAB; 38]),
                granule: 0,
            },
            Packet {
                data: vec![0x11; 100],
                granule: 100,
            },
            Packet {
                data: vec![0x22; 50],
                granule: 150,
            },
        ];
        let ogg = mux(&packets);
        assert!(is_ogg(&ogg));
        let native = to_native_flac(&ogg, false).unwrap();
        assert_eq!(native, expected_native(&packets));
    }

    #[test]
    fn packet_spanning_multiple_pages_round_trips() {
        // A packet larger than one page (255 * 255 = 65025 bytes) must split
        // across pages on encode and reassemble on decode.
        let big: Vec<u8> = (0..70_000u32).map(|i| i as u8).collect();
        let packets = vec![
            Packet {
                data: mapping_header(0, &[0xCD; 38]),
                granule: 0,
            },
            Packet {
                data: big.clone(),
                granule: 1,
            },
        ];
        let ogg = mux(&packets);
        let native = to_native_flac(&ogg, false).unwrap();
        assert_eq!(native, expected_native(&packets));
        // The big packet must really have spanned more than one page.
        let pages = ogg.windows(4).filter(|w| w == OGG_MAGIC).count();
        assert!(
            pages >= 3,
            "expected the big packet to span pages, got {pages}"
        );
    }

    #[test]
    fn headers_only_stops_after_declared_header_packets() {
        // count = 1 means one header packet (the comment) follows the mapping
        // header; the audio packet after it must not be needed by `info`.
        let packets = vec![
            Packet {
                data: mapping_header(1, &[0x01; 38]),
                granule: 0,
            },
            Packet {
                data: vec![0x84, 0, 0, 1, 0x55],
                granule: 0,
            }, // pretend comment block
            Packet {
                data: vec![0xFF; 200],
                granule: 4096,
            }, // audio frame
        ];
        let ogg = mux(&packets);
        let headers = to_native_flac(&ogg, true).unwrap();
        // Only the mapping body + the one comment packet, not the audio frame.
        let mut want = packets[0].data[MAPPING_PREFIX_LEN..].to_vec();
        want.extend_from_slice(&packets[1].data);
        assert_eq!(headers, want);
    }

    #[test]
    fn mapping_header_count_reads_field_and_validates() {
        assert_eq!(
            mapping_header_count(&mapping_header(2, &[0; 38])).unwrap(),
            2
        );
        // Wrong embedded signature is rejected.
        let mut bad = mapping_header(0, &[0; 38]);
        bad[9] = b'X'; // corrupt the `fLaC` marker
        assert!(mapping_header_count(&bad).is_err());
        // Too short is rejected, not panicked.
        assert!(mapping_header_count(&[0x7F, b'F']).is_err());
    }

    #[test]
    fn truncated_and_corrupt_pages_error_without_panic() {
        let packets = vec![
            Packet {
                data: mapping_header(0, &[0xAB; 38]),
                granule: 0,
            },
            Packet {
                data: vec![0x11; 100],
                granule: 100,
            },
        ];
        let ogg = mux(&packets);
        // Every truncation must return an error, never panic.
        for cut in 0..ogg.len() {
            let _ = to_native_flac(&ogg[..cut], false);
        }
        // Flipping a body byte must trip the page checksum.
        let mut corrupt = ogg.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0xFF;
        assert!(matches!(
            to_native_flac(&corrupt, false),
            Err(FlacError::CrcMismatch)
        ));
    }

    #[test]
    fn non_flac_ogg_is_not_flac() {
        // A BOS page whose first packet is not the FLAC mapping header.
        let packets = vec![Packet {
            data: vec![0x01; 20],
            granule: 0,
        }];
        let ogg = mux(&packets);
        assert!(matches!(
            to_native_flac(&ogg, false),
            Err(FlacError::NotFlac)
        ));
    }

    /// Build one standalone page carrying a single packet, for crafting awkward
    /// containers. `crc` overrides the checksum (to test that foreign pages are
    /// skipped without checking theirs); `None` computes the correct one.
    fn make_page(serial: u32, seq: u32, flags: u8, body: &[u8], crc: Option<u32>) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(OGG_MAGIC);
        p.push(0);
        p.push(flags);
        p.extend_from_slice(&0i64.to_le_bytes());
        p.extend_from_slice(&serial.to_le_bytes());
        p.extend_from_slice(&seq.to_le_bytes());
        let crc_at = p.len();
        p.extend_from_slice(&[0u8; 4]);
        let mut segs = Vec::new();
        let mut rem = body.len();
        loop {
            let v = rem.min(255);
            rem -= v;
            segs.push(v as u8);
            if v < 255 {
                break;
            }
        }
        p.push(segs.len() as u8);
        p.extend_from_slice(&segs);
        p.extend_from_slice(body);
        let c = crc.unwrap_or_else(|| {
            let mut q = p.clone();
            q[crc_at..crc_at + 4].fill(0);
            ogg_crc32(&q)
        });
        p[crc_at..crc_at + 4].copy_from_slice(&c.to_le_bytes());
        p
    }

    #[test]
    fn exact_multiple_of_255_packet_round_trips() {
        // A packet whose length is a multiple of 255 must end with a 0-valued
        // lacing segment, or the demuxer would read it as continuing.
        for len in [255usize, 510, 255 * 4] {
            let packets = vec![
                Packet {
                    data: mapping_header(0, &[0x09; 38]),
                    granule: 0,
                },
                Packet {
                    data: vec![0x7E; len],
                    granule: 1,
                },
            ];
            let ogg = mux(&packets);
            assert_eq!(
                to_native_flac(&ogg, false).unwrap(),
                expected_native(&packets),
                "len {len}"
            );
        }
    }

    #[test]
    fn skips_foreign_multiplexed_stream_without_checking_its_crc() {
        // A non-FLAC logical stream comes first, with a deliberately wrong
        // checksum. The demuxer must skip it by serial and still decode the
        // FLAC stream that follows.
        let flac = vec![
            Packet {
                data: mapping_header(0, &[0x42; 38]),
                granule: 0,
            },
            Packet {
                data: vec![0xAA; 80],
                granule: 100,
            },
        ];
        let foreign = make_page(
            0x1234_5678,
            0,
            FLAG_BOS,
            b"NotFLACheader",
            Some(0xDEAD_BEEF),
        );
        let mut stream = foreign;
        stream.extend_from_slice(&mux(&flac));
        assert_eq!(
            to_native_flac(&stream, false).unwrap(),
            expected_native(&flac)
        );
    }

    #[test]
    fn chained_streams_decode_the_first() {
        // Two FLAC logical streams concatenated; the demuxer stops at the first
        // stream's end-of-stream page.
        let first = vec![
            Packet {
                data: mapping_header(0, &[0x01; 38]),
                granule: 0,
            },
            Packet {
                data: vec![0x11; 40],
                granule: 10,
            },
        ];
        let second = vec![
            Packet {
                data: mapping_header(0, &[0x02; 38]),
                granule: 0,
            },
            Packet {
                data: vec![0x22; 40],
                granule: 10,
            },
        ];
        let mut stream = mux(&first);
        stream.extend_from_slice(&mux(&second));
        assert_eq!(
            to_native_flac(&stream, false).unwrap(),
            expected_native(&first)
        );
    }

    #[test]
    fn rejects_unsupported_ogg_version() {
        let mut ogg = mux(&[Packet {
            data: mapping_header(0, &[0; 38]),
            granule: 0,
        }]);
        ogg[4] = 1; // structure version must be 0
        assert!(matches!(
            to_native_flac(&ogg, false),
            Err(FlacError::Unsupported(_))
        ));
    }
}
