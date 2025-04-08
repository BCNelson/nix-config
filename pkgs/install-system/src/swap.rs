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

pub fn select_swap_size() -> Result<u64> {
    let ram_size = get_total_ram()?;

    let mut options = vec![
        format!("None (0 GB) - For systems that won't hibernate"),
        format!("Same as RAM ({} GB) - Standard for hibernation", ram_size),
        format!("Half of RAM ({} GB) - Recommended for 16GB+ systems", ram_size / 2),
        format!("Twice RAM ({} GB) - For memory-intensive workloads", ram_size * 2),
        format!("Custom size - Specify your own swap size")
    ];

    if ram_size <= 64 {
        options.insert(3, format!("Hibernation ({} GB) - RAM+2GB for safe hibernation", ram_size + 2));
    }
    
    let default_index = if ram_size <= 8 {
        1
    } else if ram_size <= 16 {
        2
    } else {
        0
    };
    
    let selection = Select::new("Select swap partition size:", options)
        .with_starting_cursor(default_index)
        .prompt()?;
    
    if selection.starts_with("None") {
        return Ok(0);
    } else if selection.starts_with("Same as RAM") {
        return Ok(ram_size);
    } else if selection.starts_with("Half of RAM") {
        return Ok(ram_size / 2);
    } else if selection.starts_with("Twice RAM") {
        return Ok(ram_size * 2);
    } else if selection.starts_with("Hibernation") {
        return Ok(ram_size + 2);
    } else {
        let custom_size = CustomType::<u64>::new("Enter custom swap size (GB):")
            .with_formatter(&|i| format!("{} GB", i))
            .with_error_message("Please enter a valid number")
            .with_help_message("Enter the swap size in GB")
            .prompt()?;
        
        return Ok(custom_size);
    }
}