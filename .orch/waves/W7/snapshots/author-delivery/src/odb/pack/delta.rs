//! git delta 格式。**T9（codex）实现范围**。
//!
//! ```text
//! base_size:  varint（7 bit/字节，bit7 续位）
//! target_size: varint
//! 指令 × n:
//!   0x00        -> 非法（保留）
//!   0x01..=0x7f -> insert：低 7 位是指令头长度，随后是 (len) 字节字面量
//!   0x80..=0xff -> copy：低 7 位是「后面还有几个参数字节」的位图，
//!                  bit0..bit3 = offset 的 4 个字节，bit4..bit6 = size 的 3 个字节
//!                  （缺省 size 为 0，即 0x10000）
//! ```
//!
//! 独立单测 + 用真实 pack 做差分，是本模块的验收方式。

use crate::error::{Error, Result};

/// copy 指令没写 size 字段时的默认长度（git 的 `0x10000`）。
const DEFAULT_COPY_SIZE: usize = 0x10000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeltaHeader {
    pub base_size: usize,
    pub target_size: usize,
    pub header_len: usize,
}

fn corrupt(detail: impl Into<String>) -> Error {
    Error::corrupt("pack delta", detail)
}

/// 读一个 7 bit/字节、bit7 续位的 varint，返回 `(value, bytes_consumed)`。
fn read_varint(data: &[u8], what: &str) -> Result<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    let mut pos = 0usize;
    loop {
        let byte = *data
            .get(pos)
            .ok_or_else(|| corrupt(format!("{what}: truncated varint")))?;
        pos += 1;
        if shift >= u64::BITS {
            return Err(corrupt(format!("{what}: varint is wider than 64 bits")));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, pos));
        }
        shift += 7;
    }
}

fn to_size(value: u64, what: &str) -> Result<usize> {
    usize::try_from(value).map_err(|_| corrupt(format!("{what} {value} does not fit in usize")))
}

pub fn parse_delta_header(delta: &[u8]) -> Result<DeltaHeader> {
    let (base_size, first_len) = read_varint(delta, "base size")?;
    let (target_size, second_len) = read_varint(&delta[first_len..], "target size")?;
    Ok(DeltaHeader {
        base_size: to_size(base_size, "base size")?,
        target_size: to_size(target_size, "target size")?,
        header_len: first_len + second_len,
    })
}

/// 把 delta 应用到 base 上，得到 target。
pub fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>> {
    let header = parse_delta_header(delta)?;
    if base.len() != header.base_size {
        return Err(corrupt(format!(
            "base size mismatch: delta header says {}, base has {} bytes",
            header.base_size,
            base.len()
        )));
    }

    let mut out: Vec<u8> = Vec::with_capacity(header.target_size.min(1 << 16));
    let mut pos = header.header_len;
    while pos < delta.len() {
        let opcode = delta[pos];
        pos += 1;
        if opcode == 0 {
            return Err(corrupt("reserved delta opcode 0x00"));
        }
        if opcode & 0x80 == 0 {
            // insert：opcode 本身就是字面量长度（1..=127）。
            let len = usize::from(opcode);
            let end = pos
                .checked_add(len)
                .ok_or_else(|| corrupt("insert length overflows"))?;
            if end > delta.len() {
                return Err(corrupt(format!(
                    "insert of {len} bytes runs past the end of the delta (only {} bytes left)",
                    delta.len() - pos
                )));
            }
            if out.len() + len > header.target_size {
                return Err(corrupt(format!(
                    "delta produces more than target_size ({}) bytes",
                    header.target_size
                )));
            }
            out.extend_from_slice(&delta[pos..end]);
            pos = end;
        } else {
            // copy：bit0..bit3 决定 offset 的 4 个字节是否出现，bit4..bit6 决定 size 的 3 个字节。
            let mut copy_offset: usize = 0;
            for i in 0..4 {
                if opcode & (1 << i) != 0 {
                    let byte = *delta
                        .get(pos)
                        .ok_or_else(|| corrupt("copy instruction is missing an offset byte"))?;
                    pos += 1;
                    copy_offset |= usize::from(byte) << (8 * i);
                }
            }
            let mut copy_size: usize = 0;
            for i in 0..3 {
                if opcode & (0x10 << i) != 0 {
                    let byte = *delta
                        .get(pos)
                        .ok_or_else(|| corrupt("copy instruction is missing a size byte"))?;
                    pos += 1;
                    copy_size |= usize::from(byte) << (8 * i);
                }
            }
            if copy_size == 0 {
                copy_size = DEFAULT_COPY_SIZE;
            }
            let end = copy_offset
                .checked_add(copy_size)
                .ok_or_else(|| corrupt("copy range overflows"))?;
            if end > base.len() {
                return Err(corrupt(format!(
                    "copy range {copy_offset}..{end} is outside the base ({} bytes)",
                    base.len()
                )));
            }
            if out.len() + copy_size > header.target_size {
                return Err(corrupt(format!(
                    "delta produces more than target_size ({}) bytes",
                    header.target_size
                )));
            }
            out.extend_from_slice(&base[copy_offset..end]);
        }
    }

    if out.len() != header.target_size {
        return Err(corrupt(format!(
            "delta produced {} bytes but its header declares target_size {}",
            out.len(),
            header.target_size
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------ 构造工具

    /// git 的 7 bit/字节 varint（bit7 续位）编码器。
    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                return out;
            }
        }
    }

    fn delta(base_size: usize, target_size: usize, instructions: &[u8]) -> Vec<u8> {
        let mut out = varint(base_size as u64);
        out.extend(varint(target_size as u64));
        out.extend_from_slice(instructions);
        out
    }

    fn assert_corrupt(delta: &[u8], base: &[u8]) {
        let result = apply_delta(base, delta);
        assert!(
            matches!(result, Err(Error::Corrupt { .. })),
            "expected Corrupt, got {result:?}"
        );
    }

    // ------------------------------------------------------------ 正向

    #[test]
    fn header_reports_sizes_and_length() {
        let mut bytes = varint(0x100);
        bytes.extend(varint(55));
        bytes.extend_from_slice(&[0x01, b'x']);
        assert_eq!(
            parse_delta_header(&bytes).unwrap(),
            DeltaHeader {
                base_size: 256,
                target_size: 55,
                header_len: 3,
            }
        );
    }

    #[test]
    fn applies_an_insert_only_delta() {
        let base = b"abc".to_vec();
        let target = b"hello world".to_vec();
        let mut instructions = vec![target.len() as u8];
        instructions.extend_from_slice(&target);
        let d = delta(base.len(), target.len(), &instructions);
        assert_eq!(apply_delta(&base, &d).unwrap(), target);
    }

    #[test]
    fn applies_a_copy_then_insert_delta() {
        let base = b"hello world\n".to_vec(); // 12 字节
                                              // 0x91 = copy，bit0（offset 低字节）+ bit4（size 低字节）：copy(0, 11)，再插 2 字节。
        let instructions = [0x91, 0x00, 0x0b, 0x02, b'!', b'!'];
        let d = delta(12, 13, &instructions);
        assert_eq!(apply_delta(&base, &d).unwrap(), b"hello world!!");
    }

    #[test]
    fn copy_size_defaults_to_0x10000() {
        let base = vec![b'A'; 0x10000 + 5];
        // 0x80 = copy，没有任何参数字节：offset 0、size 缺省 0x10000。
        let d = delta(base.len(), 0x10000, &[0x80]);
        let out = apply_delta(&base, &d).unwrap();
        assert_eq!(out.len(), 0x10000);
        assert_eq!(out, base[..0x10000]);
    }

    #[test]
    fn copy_reads_offset_and_size_as_little_endian_bytes() {
        let mut base = vec![0u8; 70_000];
        for (i, byte) in base.iter_mut().enumerate() {
            *byte = (i % 251) as u8;
        }
        let want = base[0x1234..0x1334].to_vec();
        // 0xb3 = copy：bit0/bit1 → offset 的 2 个字节，bit4/bit5 → size 的 2 个字节。
        let d = delta(base.len(), want.len(), &[0xb3, 0x34, 0x12, 0x00, 0x01]);
        assert_eq!(apply_delta(&base, &d).unwrap(), want);
    }

    #[test]
    fn accepts_a_copy_that_ends_exactly_at_the_end_of_base() {
        let base = b"0123456789".to_vec();
        let d = delta(10, 4, &[0x91, 0x06, 0x04]); // copy(6, 4)
        assert_eq!(apply_delta(&base, &d).unwrap(), b"6789");
    }

    // ------------------------------------------------------------ 反向

    #[test]
    fn rejects_the_reserved_opcode() {
        assert_corrupt(&delta(3, 1, &[0x00]), b"abc");
    }

    #[test]
    fn rejects_a_truncated_insert() {
        assert_corrupt(&delta(3, 5, &[0x05, b'a', b'b']), b"abc");
    }

    #[test]
    fn rejects_a_truncated_copy_parameter() {
        assert_corrupt(&delta(3, 5, &[0xf0, 0x00, 0x00]), b"abc");
    }

    #[test]
    fn rejects_a_copy_past_the_end_of_base() {
        // copy(0xff, 0x10000)：base 只有 3 字节。
        assert_corrupt(&delta(3, 0x10000, &[0x91, 0xff, 0x00]), b"abc");
    }

    #[test]
    fn rejects_a_base_size_mismatch() {
        assert_corrupt(&delta(4, 3, &[0x03, b'a', b'b', b'c']), b"abc");
    }

    #[test]
    fn rejects_a_delta_that_stops_short_of_target_size() {
        assert_corrupt(&delta(3, 5, &[0x02, b'a', b'b']), b"abc");
    }

    #[test]
    fn rejects_a_delta_that_overshoots_target_size() {
        assert_corrupt(&delta(3, 2, &[0x03, b'a', b'b', b'c']), b"abc");
    }

    #[test]
    fn rejects_truncated_headers_and_deltas() {
        for bytes in [Vec::new(), vec![0x80], vec![0x01], varint(3), {
            let mut bytes = varint(3);
            bytes.push(0x80);
            bytes
        }] {
            let result = parse_delta_header(&bytes);
            assert!(
                matches!(result, Err(Error::Corrupt { .. })),
                "expected Corrupt for {bytes:?}, got {result:?}"
            );
        }
    }

    #[test]
    fn rejects_a_varint_wider_than_64_bits() {
        let bytes = vec![0x80u8; 12];
        assert!(matches!(
            parse_delta_header(&bytes),
            Err(Error::Corrupt { .. })
        ));
    }
}
