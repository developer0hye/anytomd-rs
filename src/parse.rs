/// Parse a human-readable byte size string (e.g., "500MB", "2GiB", "1024").
pub(crate) fn byte_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    let (num_part, suffix) = match s.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, ""),
    };
    let num: f64 = num_part
        .trim()
        .parse()
        .map_err(|_| format!("invalid number: {num_part}"))?;
    if num.is_nan() || num.is_infinite() || num <= 0.0 {
        return Err("size must be a finite positive number".to_string());
    }
    let multiplier: u64 = match suffix.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kb" => 1_000,
        "kib" => 1_024,
        "mb" => 1_000_000,
        "mib" => 1_048_576,
        "gb" => 1_000_000_000,
        "gib" => 1_073_741_824,
        "tb" => 1_000_000_000_000,
        "tib" => 1_099_511_627_776,
        _ => return Err(format!("unknown size suffix: {suffix}")),
    };
    let bytes_f64 = num * multiplier as f64;
    if bytes_f64 >= usize::MAX as f64 {
        return Err(format!("size too large for this platform: {s}"));
    }
    let bytes = bytes_f64 as usize;
    if bytes == 0 {
        return Err("size too small to represent".to_string());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bare_number() {
        assert_eq!(byte_size("1024").unwrap(), 1024);
    }

    #[test]
    fn test_bytes_suffix() {
        assert_eq!(byte_size("512b").unwrap(), 512);
        assert_eq!(byte_size("512B").unwrap(), 512);
    }

    #[test]
    fn test_si_units() {
        assert_eq!(byte_size("1KB").unwrap(), 1_000);
        assert_eq!(byte_size("5MB").unwrap(), 5_000_000);
        assert_eq!(byte_size("2GB").unwrap(), 2_000_000_000);
        assert_eq!(byte_size("1TB").unwrap(), 1_000_000_000_000);
    }

    #[test]
    fn test_binary_units() {
        assert_eq!(byte_size("1KiB").unwrap(), 1_024);
        assert_eq!(byte_size("1MiB").unwrap(), 1_048_576);
        assert_eq!(byte_size("1GiB").unwrap(), 1_073_741_824);
        assert_eq!(byte_size("2GiB").unwrap(), 2_147_483_648);
    }

    #[test]
    fn test_fractional() {
        assert_eq!(byte_size("1.5GB").unwrap(), 1_500_000_000);
        assert_eq!(byte_size("0.5MiB").unwrap(), 524_288);
    }

    #[test]
    fn test_case_insensitive() {
        assert_eq!(byte_size("1gb").unwrap(), 1_000_000_000);
        assert_eq!(byte_size("1Gb").unwrap(), 1_000_000_000);
        assert_eq!(byte_size("1gib").unwrap(), 1_073_741_824);
    }

    #[test]
    fn test_whitespace() {
        assert_eq!(byte_size("  500 MB  ").unwrap(), 500_000_000);
        assert_eq!(byte_size("1 GiB").unwrap(), 1_073_741_824);
    }

    #[test]
    fn test_zero_rejected() {
        assert!(byte_size("0").is_err());
        assert!(byte_size("0MB").is_err());
    }

    #[test]
    fn test_negative_rejected() {
        assert!(byte_size("-1").is_err());
        assert!(byte_size("-5GB").is_err());
    }

    #[test]
    fn test_nan_rejected() {
        assert!(byte_size("NaN").is_err());
    }

    #[test]
    fn test_infinity_rejected() {
        assert!(byte_size("inf").is_err());
    }

    #[test]
    fn test_empty_rejected() {
        assert!(byte_size("").is_err());
    }

    #[test]
    fn test_no_number_rejected() {
        assert!(byte_size("MB").is_err());
    }

    #[test]
    fn test_unknown_suffix_rejected() {
        assert!(byte_size("5XB").is_err());
        assert!(byte_size("5abc").is_err());
    }

    #[test]
    fn test_overflow_rejected() {
        // 16777216 TiB = 2^64 bytes, exceeds usize::MAX on 64-bit
        assert!(byte_size("16777216TiB").is_err());
    }

    mod fuzz {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn never_panics(s in "\\PC*") {
                let _ = byte_size(&s);
            }

            #[test]
            fn valid_result_is_positive(s in "\\PC*") {
                if let Ok(v) = byte_size(&s) {
                    prop_assert!(v > 0);
                }
            }

            #[test]
            fn valid_number_with_suffix(
                n in 0.001_f64..1_000_000.0,
                suffix in prop_oneof![
                    Just(""), Just("b"), Just("B"),
                    Just("KB"), Just("kb"), Just("KiB"), Just("kib"),
                    Just("MB"), Just("mb"), Just("MiB"), Just("mib"),
                    Just("GB"), Just("gb"), Just("GiB"), Just("gib"),
                    Just("TB"), Just("tb"), Just("TiB"), Just("tib"),
                ],
            ) {
                let input = format!("{n}{suffix}");
                let result = byte_size(&input);
                prop_assert!(result.is_ok(), "expected Ok for {input:?}, got {result:?}");
                prop_assert!(result.unwrap() > 0);
            }

            #[test]
            fn roundtrip_whole_bytes(n in 1_usize..=1_000_000) {
                let input = format!("{n}b");
                prop_assert_eq!(byte_size(&input).unwrap(), n);
            }
        }
    }
}
