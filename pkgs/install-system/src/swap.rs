use anyhow::{Result, bail};
use inquire::{Select, CustomType};
use std::fs;

/// Get the total RAM size in GB
fn get_total_ram() -> Result<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo")?;
    
    for line in meminfo.lines() {
        if line.starts_with("MemTotal:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(kb) = parts[1].parse::<u64>() {
                    // Convert to GB and round up
                    let gb = (kb + 1048575) / 1048576; // 1048576 = 1024*1024 (kB to GB)
                    return Ok(gb);
                }
            }
            bail!("Failed to parse MemTotal value");
        }
    }
    
    bail!("MemTotal not found in /proc/meminfo")
}

/// The ESP that disko/default.nix carves out before anything else.
const ESP_GB: u64 = 1;

/// Floor we refuse to let swap eat into.
///
/// A thin client's system closure is ~1.2 GB, and an update needs the running
/// generation plus the one being fetched, with enough left for /var and the
/// journal. 5 GB covers that with room for a third generation to roll back to.
///
/// It is deliberately not higher: at 6 GB an 8 GB eMMC (7 GiB usable) can hold
/// no swap at all, and having the option of a gigabyte is worth more than the
/// extra margin. It is also not a measurement -- nobody has watched an update
/// fail at 4 GB -- so treat it as a considered floor rather than a hard limit.
const MIN_ROOT_GB: u64 = 5;

/// Usable capacity of a disk, in whole GB, from sysfs.
pub fn disk_size_gb(disk: &std::path::Path) -> Result<u64> {
    let name = disk
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("not a device path: {}", disk.display()))?;
    let sectors: u64 = fs::read_to_string(format!("/sys/block/{}/size", name))?
        .trim()
        .parse()?;
    // sysfs reports 512-byte sectors regardless of the physical sector size.
    Ok(sectors * 512 / 1_073_741_824)
}

/// Ask for a swap size, offering only sizes that actually fit on the target.
///
/// The sizes worth having are a function of RAM, but whether they are *possible*
/// is a function of the disk -- and on a thin client with an 8 GB eMMC those
/// disagree sharply. Offering "twice RAM" on a disk that cannot hold it means
/// the install dies inside disko, after the partition table has already been
/// written.
/// Largest swap that leaves the ESP and the root floor intact.
///
/// saturating_sub matters: a disk smaller than ESP + floor yields 0 rather than
/// wrapping to an enormous number and offering to fill a disk that cannot hold
/// it. Split out from the prompt so the arithmetic can be tested.
fn max_swap_gb(disk_gb: u64) -> u64 {
    disk_gb.saturating_sub(ESP_GB + MIN_ROOT_GB)
}

pub fn select_swap_size(disk_gb: u64) -> Result<u64> {
    let ram_size = get_total_ram()?;
    let max_swap = max_swap_gb(disk_gb);

    // (label, size in GB). None is always available.
    let mut candidates: Vec<(String, u64)> = vec![
        ("Same as RAM - standard for hibernation".to_string(), ram_size),
        ("Half of RAM".to_string(), ram_size / 2),
        ("Twice RAM - for memory-intensive workloads".to_string(), ram_size * 2),
    ];
    if ram_size <= 64 {
        candidates.push(("Hibernation - RAM+2GB for safe hibernation".to_string(), ram_size + 2));
    }

    let dropped: Vec<&(String, u64)> = candidates.iter().filter(|(_, gb)| *gb > max_swap).collect();
    if !dropped.is_empty() {
        println!(
            "\n{} GB disk: at most {} GB of swap fits, leaving {} GB for the ESP and {} GB for the system.",
            disk_gb, max_swap, ESP_GB, MIN_ROOT_GB
        );
        for (label, gb) in &dropped {
            println!("  not offered: {} GB - {}", gb, label);
        }
    }

    let mut options: Vec<String> = vec!["None (0 GB) - for systems that won't hibernate".to_string()];
    let mut values: Vec<u64> = vec![0];
    // Dedupe by size: on a 2 GB machine "twice RAM" and "RAM+2GB" are both
    // 4 GB, and offering the same number twice just invites the reader to look
    // for a difference that is not there.
    for (label, gb) in candidates.iter().filter(|(_, gb)| *gb > 0 && *gb <= max_swap) {
        if values.contains(gb) {
            continue;
        }
        options.push(format!("{} GB - {}", gb, label));
        values.push(*gb);
    }
    if max_swap > 0 {
        options.push(format!("Custom size (up to {} GB)", max_swap));
    }

    // Same intent as before: hibernation-sized on small-RAM machines, none on
    // large ones -- but only when that option survived the disk check.
    let preferred = if ram_size <= 8 { ram_size } else if ram_size <= 16 { ram_size / 2 } else { 0 };
    let default_index = values.iter().position(|v| *v == preferred).unwrap_or(0);

    let selection = Select::new("Select swap partition size:", options.clone())
        .with_starting_cursor(default_index)
        .prompt()?;

    let index = options.iter().position(|o| *o == selection).unwrap();
    if index < values.len() {
        return Ok(values[index]);
    }

    // Custom: keep asking until it fits, rather than failing later inside disko.
    loop {
        let custom = CustomType::<u64>::new("Enter custom swap size (GB):")
            .with_formatter(&|i| format!("{} GB", i))
            .with_error_message("Please enter a valid number")
            .with_help_message(&format!("Between 0 and {} GB on this disk", max_swap))
            .prompt()?;
        if custom <= max_swap {
            return Ok(custom);
        }
        println!(
            "{} GB will not fit: this disk is {} GB and needs {} GB for the ESP plus {} GB for the system.",
            custom, disk_gb, ESP_GB, MIN_ROOT_GB
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_emmc_leaves_room_for_a_little_swap() {
        // A "8 GB" Wyse 3040 eMMC is ~7 GiB once you measure it honestly.
        assert_eq!(max_swap_gb(7), 1);
    }

    #[test]
    fn sixteen_gb_emmc_fits_a_hibernation_sized_swap() {
        // 14 GiB usable; 2 GB of RAM means hibernation wants 4 GB.
        assert!(max_swap_gb(14) >= 4);
    }

    #[test]
    fn a_disk_too_small_for_the_floor_offers_no_swap_rather_than_wrapping() {
        // The subtraction underflows here; saturating_sub is what stops this
        // becoming u64::MAX and offering to fill a disk that cannot hold it.
        for tiny in [0, 1, ESP_GB + MIN_ROOT_GB - 1, ESP_GB + MIN_ROOT_GB] {
            assert_eq!(max_swap_gb(tiny), 0, "disk_gb={}", tiny);
        }
    }

    #[test]
    fn large_disk_is_bounded_only_by_the_reserved_space() {
        assert_eq!(max_swap_gb(512), 512 - ESP_GB - MIN_ROOT_GB);
    }
}
