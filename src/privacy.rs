//! Padding/decoy — chống suy luận loại/độ nhạy qua KÍCH THƯỚC (INV-E9 mức code).
//!
//! INV-E5 giấu loại khỏi định danh; nhưng kích thước byte vẫn có thể tiết lộ loại
//! (PDF nhỏ vs video lớn). [`pad_to_bucket`] làm tròn lên bucket cố định để mọi
//! object cùng bucket trông giống nhau; [`decoy_sizes`] sinh kích thước nhiễu tất
//! định để rải traffic giả. Hàm THUẦN, có test.

/// Các bucket cố định (byte): 4KB, 16KB, 64KB, 256KB, 1MB. Object lớn hơn 1MB
/// được làm tròn lên bội của 1MB (giữ rò rỉ ở mức thô).
pub const BUCKETS: [usize; 5] = [4096, 16_384, 65_536, 262_144, 1_048_576];

/// Làm tròn `len` LÊN bucket cố định nhỏ nhất ≥ len. Nếu > bucket lớn nhất, làm
/// tròn lên bội của bucket lớn nhất.
pub fn pad_to_bucket(len: usize) -> usize {
    for &b in &BUCKETS {
        if len <= b {
            return b;
        }
    }
    let top = BUCKETS[BUCKETS.len() - 1];
    len.div_ceil(top) * top
}

/// Số byte padding cần thêm để đạt bucket.
pub fn padding_len(len: usize) -> usize {
    pad_to_bucket(len) - len
}

/// Sinh `count` kích thước decoy (nhiễu) TẤT ĐỊNH từ `seed` — chọn trong các bucket
/// để decoy không phân biệt được với object thật theo kích thước. Tất định để có thể
/// tái lập/kiểm thử (không phụ thuộc RNG hệ thống).
pub fn decoy_sizes(seed: u64, count: usize) -> Vec<usize> {
    // PRNG tất định nhỏ (splitmix64) — chỉ để rải decoy, KHÔNG dùng cho mật mã.
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    (0..count)
        .map(|_| BUCKETS[(next() as usize) % BUCKETS.len()])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_up_to_smallest_fitting_bucket() {
        assert_eq!(pad_to_bucket(0), 4096);
        assert_eq!(pad_to_bucket(1), 4096);
        assert_eq!(pad_to_bucket(4096), 4096);
        assert_eq!(pad_to_bucket(4097), 16_384);
        assert_eq!(pad_to_bucket(60_000), 65_536);
        assert_eq!(pad_to_bucket(1_048_576), 1_048_576);
    }

    #[test]
    fn large_rounds_to_multiple_of_top() {
        assert_eq!(pad_to_bucket(1_048_577), 2 * 1_048_576);
        assert_eq!(pad_to_bucket(3_000_000), 3 * 1_048_576);
    }

    #[test]
    fn padding_len_consistent() {
        assert_eq!(padding_len(1), 4095);
        assert_eq!(padding_len(4096), 0);
        assert_eq!(padding_len(5000), 16_384 - 5000);
    }

    #[test]
    fn different_types_same_bucket_indistinguishable() {
        // Một "PDF" 3KB và một "ảnh" 3.5KB → cùng bucket 4KB → không phân biệt được loại.
        assert_eq!(pad_to_bucket(3000), pad_to_bucket(3500));
    }

    #[test]
    fn decoy_deterministic_and_in_buckets() {
        let a = decoy_sizes(42, 16);
        let b = decoy_sizes(42, 16);
        assert_eq!(a, b, "tất định theo seed");
        assert_eq!(a.len(), 16);
        for s in a {
            assert!(BUCKETS.contains(&s), "decoy phải thuộc bucket hợp lệ");
        }
        assert_ne!(decoy_sizes(42, 8), decoy_sizes(43, 8), "seed khác → dãy khác");
    }
}
