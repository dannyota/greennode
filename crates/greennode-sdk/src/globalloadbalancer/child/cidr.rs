use std::{collections::HashSet, net::Ipv4Addr};

use super::{Error, refuse};

pub(super) fn prefix(text: &str) -> Option<(u32, u32)> {
    let (ip, len) = text.split_once('/')?;
    let ip: Ipv4Addr = ip.parse().ok()?;
    let bits: u32 = len.parse().ok()?;
    (bits <= 32 && bits.to_string() == len).then(|| (u32::from(ip), bits))
}

pub(super) fn masked(ip: u32, len: u32) -> u32 {
    u32::MAX.checked_shl(32 - len).map_or(0, |mask| ip & mask)
}

fn canonical_prefix(text: &str) -> Option<(u32, u32)> {
    let (ip, bits) = prefix(text)?;
    (bits >= 8 && masked(ip, bits) == ip).then_some((ip, bits))
}

pub(super) fn canonical(op: &str, items: &[String]) -> Result<String, Box<Error>> {
    if items.is_empty() {
        return Err(refuse(&format!("{op} requires at least one allowed CIDR")));
    }
    let mut unique = HashSet::new();
    let mut spans = Vec::with_capacity(items.len());
    for item in items {
        let (ip, len) = canonical_prefix(item).ok_or_else(|| {
            refuse(&format!(
                "{op} allowed CIDRs must be canonical IPv4 networks from /8 through /32"
            ))
        })?;
        if !unique.insert((ip, len)) {
            return Err(refuse(&format!("{op} allowed CIDRs repeat a prefix")));
        }
        let start = u64::from(ip);
        spans.push((start, start + (1 << (32 - len))));
    }
    spans.sort_unstable();
    let mut covered = 0;
    for (start, end) in spans {
        if start > covered {
            break;
        }
        covered = covered.max(end);
    }
    if covered == 1 << 32 {
        return Err(refuse(&format!(
            "{op} allowed CIDRs may not cover all IPv4 addresses"
        )));
    }
    Ok(items.join(","))
}

pub(super) fn same_set(got: &str, want: &[String]) -> bool {
    let expected: HashSet<_> = want.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    got.split(',')
        .all(|part| expected.contains(part) && seen.insert(part))
        && seen.len() == expected.len()
}
