// wifi.rs
use anyhow::{Result, bail};
use cmd_lib::{run_cmd, run_fun};
use std::time::Duration;

const MAX_RETRIES: u32 = 3;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

struct WifiCredentials {
    ssid: String,
    password: String,
}

fn get_wifi_credentials() -> Result<WifiCredentials> {
    run_cmd!(nmcli device wifi list --rescan yes)?;

    let ssid = inquire::Text::new("Enter WiFi SSID:").prompt()?;
    let password = inquire::Password::new("Enter WiFi password:").prompt()?;

    Ok(WifiCredentials {
        ssid: ssid.trim().to_string(),
        password: password.trim().to_string(),
    })
}

fn check_internet_connection() -> bool {
    run_cmd!(ping -c 1 -W 5 8.8.8.8).is_ok()
}

fn check_wifi_hardware() -> Result<String> {
    let interfaces = run_fun!(ls /sys/class/net)?;
    if !interfaces.split_whitespace().any(|iface| iface.starts_with("wl")) {
        bail!("No wireless interface found");
    }

    let output = run_fun!(ls /sys/class/net | grep ^wl)?;
    let interface = output.trim();
    
    let rfkill_output = run_fun!(rfkill list wifi)?;
    if rfkill_output.contains("Soft blocked: yes") || rfkill_output.contains("Hard blocked: yes") {
        println!("WiFi is blocked, attempting to unblock...");
        run_cmd!(sudo rfkill unblock wifi)?;
        
        let rfkill_check = run_fun!(rfkill list wifi)?;
        if rfkill_check.contains("Soft blocked: yes") || rfkill_check.contains("Hard blocked: yes") {
            bail!("Failed to unblock WiFi - might be physically disabled");
        }
    }

    Ok(interface.to_string())
}

fn connect_wifi(credentials: &WifiCredentials) -> Result<()> {
    let _ = run_cmd!(sudo nmcli radio wifi on);
    let ssid = credentials.ssid.clone();
    let password = credentials.password.clone();
    let _ = run_cmd!(sudo nmcli dev wifi connect $ssid password $password)?;

    Ok(())
}

pub fn ensure_connectivity() -> Result<()> {
    if check_internet_connection() {
        return Ok(());
    }

    println!("No internet connection detected. Checking WiFi hardware...");
    
    let interface = match check_wifi_hardware() {
        Ok(iface) => iface,
        Err(e) => bail!("WiFi hardware check failed: {}", e)
    };

    println!("Found wireless interface: {}", interface);

    for attempt in 1..=MAX_RETRIES {
        if attempt > 1 {
            println!("\nRetrying WiFi connection (attempt {}/{})", attempt, MAX_RETRIES);
        }

        let credentials = get_wifi_credentials()?;
        if let Err(e) = connect_wifi(&credentials) {
            println!("Error setting up WiFi: {}", e);
            continue;
        }

        let start = std::time::Instant::now();
        while start.elapsed() < CONNECTION_TIMEOUT {
            if check_internet_connection() {
                println!("Successfully connected to WiFi");
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        
        println!("Connection attempt timed out");
    }

    bail!("Failed to establish WiFi connection after {} attempts", MAX_RETRIES)
}