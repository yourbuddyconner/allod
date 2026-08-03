//! Canonical wire encoding (§7.1): CBOR per RFC 8949 Core
//! Deterministic Encoding. Definite lengths, map keys sorted by their
//! encoded bytes, shortest-form integers. All hashes and signatures
//! are computed over this encoding, so every byte here is normative.

use serde_yaml::Value;

/// Encode a value as canonical CBOR. Map keys must be strings.
/// Floats are rejected until the shortest-encoding rules land with
/// their own test vectors; nothing hashed today contains one.
pub fn canonical_cbor(value: &Value) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    encode(value, &mut out)?;
    Ok(out)
}

fn encode(value: &Value, out: &mut Vec<u8>) -> Result<(), String> {
    match value {
        Value::Null => {
            out.push(0xf6);
            Ok(())
        }
        Value::Bool(b) => {
            out.push(if *b { 0xf5 } else { 0xf4 });
            Ok(())
        }
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                encode_head(0, u, out);
                Ok(())
            } else if let Some(i) = n.as_i64() {
                // Negative: major type 1 encodes -(n + 1).
                encode_head(1, (-(i + 1)) as u64, out);
                Ok(())
            } else {
                Err(format!("float {n} has no canonical encoding yet"))
            }
        }
        Value::String(s) => {
            encode_head(3, s.len() as u64, out);
            out.extend_from_slice(s.as_bytes());
            Ok(())
        }
        Value::Sequence(seq) => {
            encode_head(4, seq.len() as u64, out);
            for item in seq {
                encode(item, out)?;
            }
            Ok(())
        }
        Value::Mapping(map) => {
            let mut entries = Vec::with_capacity(map.len());
            for (key, val) in map {
                let Some(key) = key.as_str() else {
                    return Err(format!("map key {key:?} must be a string"));
                };
                let mut kbytes = Vec::new();
                encode_head(3, key.len() as u64, &mut kbytes);
                kbytes.extend_from_slice(key.as_bytes());
                entries.push((kbytes, val));
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            encode_head(5, entries.len() as u64, out);
            for (kbytes, val) in entries {
                out.extend_from_slice(&kbytes);
                encode(val, out)?;
            }
            Ok(())
        }
        Value::Tagged(t) => Err(format!("tagged value {t:?} has no canonical encoding")),
    }
}

/// Shortest-form head: major type plus argument (RFC 8949 §4.2.1).
fn encode_head(major: u8, arg: u64, out: &mut Vec<u8>) {
    let m = major << 5;
    if arg < 24 {
        out.push(m | arg as u8);
    } else if arg <= u8::MAX as u64 {
        out.push(m | 24);
        out.push(arg as u8);
    } else if arg <= u16::MAX as u64 {
        out.push(m | 25);
        out.extend_from_slice(&(arg as u16).to_be_bytes());
    } else if arg <= u32::MAX as u64 {
        out.push(m | 26);
        out.extend_from_slice(&(arg as u32).to_be_bytes());
    } else {
        out.push(m | 27);
        out.extend_from_slice(&arg.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cbor(yaml: &str) -> Vec<u8> {
        canonical_cbor(&serde_yaml::from_str(yaml).unwrap()).unwrap()
    }

    #[test]
    fn known_encodings() {
        assert_eq!(cbor("0"), [0x00]);
        assert_eq!(cbor("23"), [0x17]);
        assert_eq!(cbor("24"), [0x18, 0x18]);
        assert_eq!(cbor("500"), [0x19, 0x01, 0xf4]);
        assert_eq!(cbor("-1"), [0x20]);
        assert_eq!(cbor("true"), [0xf5]);
        assert_eq!(cbor("null"), [0xf6]);
        assert_eq!(cbor("\"a\""), [0x61, 0x61]);
        assert_eq!(cbor("[2, 3]"), [0x82, 0x02, 0x03]);
    }

    #[test]
    fn maps_sort_by_encoded_key() {
        // RFC 8949 appendix example shape: {"a": 1, "b": [2, 3]}.
        assert_eq!(
            cbor("{ b: [2, 3], a: 1 }"),
            [0xa2, 0x61, 0x61, 0x01, 0x61, 0x62, 0x82, 0x02, 0x03]
        );
        // Shorter keys sort before longer ones (length-first bytes).
        assert_eq!(
            cbor("{ aa: 2, b: 1 }"),
            [0xa2, 0x61, 0x62, 0x01, 0x62, 0x61, 0x61, 0x02]
        );
    }

    #[test]
    fn rejects_floats_and_nonstring_keys() {
        assert!(canonical_cbor(&serde_yaml::from_str("1.5").unwrap()).is_err());
        assert!(canonical_cbor(&serde_yaml::from_str("{ 1: a }").unwrap()).is_err());
    }
}
