use super::pos_stats;

fn reference(mut v: Vec<u32>) -> (Option<u32>, Option<u32>, Option<u32>) {
    if v.is_empty() {
        return (None, None, None);
    }
    v.sort_unstable();
    (Some(v[0]), Some(v[v.len() / 2]), Some(v[v.len() - 1]))
}

#[test]
fn matches_the_sorted_reference_on_every_small_permutation() {
    use std::collections::BTreeSet;
    let pools: [&[u32]; 4] = [&[5], &[3, 9], &[7, 7, 2, 9], &[1, 2, 2, 8, 5]];
    for pool in pools {
        let mut perms: BTreeSet<Vec<u32>> = BTreeSet::new();
        permute(&mut pool.to_vec(), 0, &mut perms);
        for p in perms {
            assert_eq!(pos_stats(p.clone()), reference(p.clone()), "{p:?}");
        }
    }
    assert_eq!(pos_stats(Vec::new()), (None, None, None));
}

fn permute(v: &mut Vec<u32>, k: usize, out: &mut std::collections::BTreeSet<Vec<u32>>) {
    if k == v.len() {
        out.insert(v.clone());
        return;
    }
    for i in k..v.len() {
        v.swap(k, i);
        permute(v, k + 1, out);
        v.swap(k, i);
    }
}

#[test]
fn the_median_of_an_even_count_is_the_upper_one() {
    assert_eq!(pos_stats(vec![3, 5, 7, 9]), (Some(3), Some(7), Some(9)));
}
