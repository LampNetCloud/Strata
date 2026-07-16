//! Merkle Sum Tree cho dữ liệu tabular — tổng/đếm một cột CÓ bằng chứng (Feat §8,
//! Math §14).
//!
//! Bảng là rừng row-Strata (granularity per-row): mỗi hàng là một Strata #4, mỗi
//! cột là một trường trong `state_root`. Để chứng minh "tổng cột lương = 5 tỷ" mà
//! không lộ từng hàng, ta dựng một **Merkle Sum Tree (MST)** RIÊNG trên cột cần
//! tổng — KHÔNG trộn vào `state_root` hồ sơ cấu trúc (mỗi cây một miền, INV-E8).
//!
//! MST là sub-primitive dùng chung với chuỗi đo lường A22 của VeData, cài một lần
//! trong [`lampnet_merkle_anchor::sumtree`] (hash-agnostic). Strata dùng
//! `<Blake3Hasher>`; tag riêng `LN/STRATA/sum/{leaf,node}/v1` (CHỐT-2) đã tách miền
//! hoàn toàn khỏi cây state/MMR.
//!
//! Mỗi node MST giữ thêm `(sum, count)` được BĂM vào node, nên không sửa được mà
//! giữ root. Proof một hàng lộ giá trị hàng đó + các `(sum, count)` anh em (tổng cục
//! bộ) — đủ audit tổng, yếu hơn ZK đầy đủ (muốn giấu tổng cục bộ cần blinding/ZK,
//! backlog như Math §6.3/§14).

use lampnet_merkle_anchor::Blake3Hasher;
use lampnet_merkle_anchor::hash::Hash32;
use lampnet_merkle_anchor::sumtree::{Node, SumRangeProof, SumTree, verify_sum_range};

/// Cây tổng một cột của một bảng. `row_key` = định danh hàng (ví dụ `ref_id` hàng
/// 32B thuần — CHỐT-4); `value` = giá trị ô của cột đó ở hàng (u128, đủ cho tiền tệ
/// lovelace/đơn vị nhỏ, tránh số thực bất định).
///
/// `SumTree<H>` derive Clone/Debug yêu cầu `H: Clone+Debug` (PhantomData), mà
/// `Blake3Hasher` là unit struct không derive — nên hand-impl Debug/Clone qua `rows`
/// (tái dựng cây từ hàng), như `StrataChain`/`AuditLog`.
pub struct ColumnSumTree {
    rows: Vec<(Vec<u8>, u128)>,
    tree: SumTree<Blake3Hasher>,
}

impl std::fmt::Debug for ColumnSumTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnSumTree")
            .field("n_rows", &self.rows.len())
            .field("root", &self.tree.root())
            .field("total", &self.tree.root_node().sum)
            .finish()
    }
}

impl Clone for ColumnSumTree {
    fn clone(&self) -> Self {
        Self {
            rows: self.rows.clone(),
            tree: SumTree::<Blake3Hasher>::build(&self.rows),
        }
    }
}

/// Bằng chứng một hàng góp đúng phần vào tổng cột. Bọc `SumRangeProof` của dải một
/// phần tử `[i, i+1)` + siêu dữ liệu để verify độc lập (stateless).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowSumProof {
    /// Vị trí hàng trong cây (0-based, theo thứ tự dựng cây).
    pub row_index: usize,
    /// Giá trị ô của hàng tại cột này.
    pub value: u128,
    /// Số hàng của cả bảng (cần cho verify — MST commit `count` ở gốc).
    pub n_rows: usize,
    /// Proof dải `[row_index, row_index+1)`.
    pub proof: SumRangeProof,
}

impl ColumnSumTree {
    /// Dựng cây tổng cột từ danh sách `(row_key, value)`. Thứ tự hàng = thứ tự đưa
    /// vào (caller tự chọn thứ tự tất định — ví dụ sort theo `row_key`).
    pub fn build(rows: &[(Vec<u8>, u128)]) -> Self {
        Self {
            rows: rows.to_vec(),
            tree: SumTree::<Blake3Hasher>::build(rows),
        }
    }

    /// Số hàng.
    pub fn len(&self) -> usize {
        self.tree.len()
    }

    /// Bảng rỗng?
    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// Root hash cam kết cả cột (đưa vào state của Strata index, hoặc neo riêng).
    pub fn root(&self) -> Hash32 {
        self.tree.root()
    }

    /// Root node đầy đủ `(hash, sum, count)` — `sum` = tổng cột, `count` = số hàng.
    pub fn root_node(&self) -> Node {
        self.tree.root_node()
    }

    /// Tổng cả cột (`total_sum` ở gốc MST).
    pub fn total(&self) -> u128 {
        self.tree.root_node().sum
    }

    /// Tổng một dải hàng `[a, b)` (không proof — tiện tra cứu cục bộ).
    pub fn sum_range(&self, a: usize, b: usize) -> u128 {
        self.tree.sum_range(a, b)
    }

    /// Sinh bằng chứng một hàng góp vào tổng. `None` nếu `row_index` ngoài phạm vi.
    pub fn prove_row(&self, row_index: usize) -> Option<RowSumProof> {
        if row_index >= self.tree.len() {
            return None;
        }
        Some(RowSumProof {
            row_index,
            value: self.tree.sum_range(row_index, row_index + 1),
            n_rows: self.tree.len(),
            proof: self.tree.prove_sum_range(row_index, row_index + 1),
        })
    }

    /// Sinh bằng chứng tổng một DẢI hàng `[a, b)` (ví dụ "tổng 100 hàng đầu = X").
    /// `None` nếu dải không hợp lệ.
    pub fn prove_range(&self, a: usize, b: usize) -> Option<RowSumProof> {
        if !(a < b && b <= self.tree.len()) {
            return None;
        }
        Some(RowSumProof {
            row_index: a,
            value: self.tree.sum_range(a, b),
            n_rows: self.tree.len(),
            proof: self.tree.prove_sum_range(a, b),
        })
    }
}

/// Verify một [`RowSumProof`] dưới `root` đã cam kết. Trả `true` nếu cây tái dựng
/// khớp `root`, tổng dải khớp `value`, và `count` gốc khớp `n_rows`.
pub fn verify_row_sum(root: Hash32, proof: &RowSumProof) -> bool {
    // Một hàng đơn = dải một phần tử; một dải = [row_index, row_index + width).
    // Với prove_row width==1; với prove_range width>=1. Suy width từ proof? Không —
    // RowSumProof chỉ giữ điểm đầu; ta verify dải [row_index, row_index+1) cho hàng
    // đơn. Để hỗ trợ dải rộng, dùng verify_range.
    verify_row_sum_range(root, proof.row_index, proof.row_index + 1, proof)
}

/// Verify tổng một dải `[a, b)`. `value` trong proof là tổng dải; `n_rows` là số lá.
pub fn verify_row_sum_range(root: Hash32, a: usize, b: usize, proof: &RowSumProof) -> bool {
    verify_sum_range::<Blake3Hasher>(root, a, b, proof.n_rows, proof.value, &proof.proof)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<(Vec<u8>, u128)> {
        vec![
            (b"row_alice".to_vec(), 5_000),
            (b"row_bob".to_vec(), 3_000),
            (b"row_carol".to_vec(), 12_000),
            (b"row_dave".to_vec(), 8_000),
            (b"row_eve".to_vec(), 1_000),
        ]
    }

    /// Test #17 (Tech §9): tổng cột = total_sum ở gốc; proof một hàng cộng dồn ra
    /// đúng tổng; sửa giá trị hàng KHÔNG giữ được root.
    #[test]
    fn merkle_sum_tree_total() {
        let t = ColumnSumTree::build(&rows());
        // Tổng cột = 5+3+12+8+1 nghìn = 29_000.
        assert_eq!(t.total(), 29_000);
        assert_eq!(t.root_node().count, 5);

        // Proof mỗi hàng góp đúng phần vào tổng.
        let root = t.root();
        for i in 0..t.len() {
            let p = t.prove_row(i).expect("proof hàng tồn tại");
            assert!(verify_row_sum(root, &p), "verify hàng {i}");
        }

        // Sửa giá trị một hàng → root đổi → proof cũ fail dưới root cũ.
        let mut tampered = rows();
        tampered[2].1 = 999_999; // carol khai man
        let t2 = ColumnSumTree::build(&tampered);
        assert_ne!(t2.root(), root, "sửa giá trị phải đổi root");
        assert_ne!(t2.total(), t.total());
    }

    #[test]
    fn row_proof_wrong_value_fails() {
        let t = ColumnSumTree::build(&rows());
        let root = t.root();
        let mut p = t.prove_row(1).unwrap();
        p.value = 999; // khai man giá trị bob
        assert!(!verify_row_sum(root, &p));
    }

    #[test]
    fn range_sum_proof() {
        let t = ColumnSumTree::build(&rows());
        let root = t.root();
        // Tổng 3 hàng đầu = 5+3+12 = 20 nghìn.
        let p = t.prove_range(0, 3).unwrap();
        assert_eq!(p.value, 20_000);
        assert!(verify_row_sum_range(root, 0, 3, &p));
        // Khai man tổng dải → fail.
        let mut bad = p.clone();
        bad.value = 21_000;
        assert!(!verify_row_sum_range(root, 0, 3, &bad));
    }

    #[test]
    fn out_of_range_none() {
        let t = ColumnSumTree::build(&rows());
        assert!(t.prove_row(5).is_none());
        assert!(t.prove_range(3, 3).is_none());
        assert!(t.prove_range(0, 6).is_none());
    }

    #[test]
    fn single_row_table() {
        let t = ColumnSumTree::build(&[(b"only".to_vec(), 42)]);
        assert_eq!(t.total(), 42);
        let p = t.prove_row(0).unwrap();
        assert!(verify_row_sum(t.root(), &p));
    }

    #[test]
    fn deterministic_root() {
        // Cùng hàng cùng thứ tự → cùng root trên mọi máy.
        assert_eq!(
            ColumnSumTree::build(&rows()).root(),
            ColumnSumTree::build(&rows()).root()
        );
    }
}
