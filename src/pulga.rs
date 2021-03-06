// TODO: /cpu/procinfo quirks
//     * Intel usually puts an @ with the frequency in `model name`
//     * AMD usually puts something like "Eight-Core Processor" in `model name`
//       (at least in the Ryzen series)
//     * `model nome` is really vague in Raspberry Pis. Getting `Hardware` would
//       be a better fit.

use crate::{
    screenres::get_screen_resolution,
    sysinfo::SysInfo,
    uname::UnameData,
    util::{char_ptr_to_string, os_str_to_string, get_base},
};

#[cfg(feature = "use_xlib")]
use crate::screenresx11;

use libc::{c_char, gethostname, getpwuid_r, getuid, passwd, sysconf};

use smallvec::{smallvec, SmallVec};

use std::{cmp, env, fs, mem, ptr};

#[derive(Debug)]
pub struct UserData {
    pub username:       String, // User's username
    pub hostname:       String, // User's hostname
    pub cpu_info:       String, // Some CPU info
    pub cwd:            String, // User's current working directory. TODO: unneeded?
    pub hmd:            String, // User's home directory
    pub shell:          String, // User's standard shell
    pub desk_env:       String, // User's desktop environment
    // pub distro_id:      String, // User's distro ID name
    pub distro:         String, // User's distro's pretty name
    pub uptime:         String, // Time elapsed since boot
    pub editor:         String, // User's default editor, as pointed by the EDITOR var env.
    pub kernel_version: String, // User's current kernel version
    pub total_memory:   String, // Total memory in human-readable form
    pub used_memory:    String, // Used memory in human-readable form
    pub monitor_res:    String, // Resolution of currently connected monitors.
}

/// The number of threads the CPU can handle at any given time
fn get_logical_cpus() -> usize {
    use libc::{cpu_set_t, sched_getaffinity, _SC_NPROCESSORS_ONLN};

    let mut set: cpu_set_t = unsafe { mem::zeroed() };
    let code = unsafe { sched_getaffinity(0, mem::size_of::<cpu_set_t>(), &mut set) };

    // If sched_getaffinity returns 0 (succeeded)
    if code == 0 {
        let mut count = 0;
        for i in 0..libc::CPU_SETSIZE as usize {
            if unsafe { libc::CPU_ISSET(i, &set) } {
                count += 1
            }
        }
        count
    } else {
        let cpus = unsafe { sysconf(_SC_NPROCESSORS_ONLN) };
        cmp::max(1, cpus) as usize
    }
}

pub fn get_cpu_max_freq() -> Option<String> {
    let scaling_max_freq_str =
        match std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq") {
            Ok(freq) => freq,
            Err(_) => return None,
        };

    let max_freq_hz: usize = scaling_max_freq_str.trim().parse().ok()?;

    let max_freq_ghz = (max_freq_hz as f64) / 1000000.0;

    Some(format!("{:.2} GHz", max_freq_ghz))
}

/// pretty_bytes gets a value in bytes and returns a human-readable form of it
fn pretty_bytes(num: f64) -> String {
    let negative = if num < 0.0 { "-" } else { "" };
    let num = num.abs();

    const UNITS: &[&str] = &["B", "kB", "MB", "GB", "TB"];
    if num < 1.0 {
        return format!("{}{} {}", negative, num, "B");
    }
    let v1 = (num.ln() / 1024_f64.ln()).floor() as i32;
    let exponent = cmp::min(v1, 4_i32);
    let pretty_bytes = format!("{:.2}", num / 1024_f64.powi(exponent));
    let unit: &str = UNITS[exponent as usize];

    format!("{}{} {}", negative, pretty_bytes, unit)
}

/// get_user_data returns a new UserData structure
pub fn get_user_data() -> UserData {
    let (username, home_dir, shell) = if let Some(res) = get_username_home_dir_and_shell() {
        res
    } else {
        let unknown = "Unknown".to_string();
        (unknown.clone(), unknown.clone(), unknown)
    };

    // Current working directory
    let cwd: String = os_str_to_string(env::current_dir().unwrap().as_ref());

    let uname_data = UnameData::gather();

    let hostname = get_hostname().unwrap_or_else(|| "Unknown".to_string());
    let distro = get_distro().unwrap_or_else(|| "Linux".to_string());

    let sys_info = SysInfo::gather();

    #[cfg(feature = "use_xlib")]
    let resolution = unsafe { screenresx11::get_screen_resolution().join(" ") };

    #[cfg(not(feature = "use_xlib"))]
    let resolution = get_screen_resolution().unwrap_or_else(|| "Unknown".to_string());

    UserData {
        username,
        hostname,
        cpu_info: format!(
            "{} - {}x {}",
            get_cpu_model().unwrap_or_else(|| "Unknown".to_string()),
            get_logical_cpus(),
            get_cpu_max_freq().unwrap_or_else(|| "Unknown Freq.".to_string()),
        ),
        cwd,
        hmd: home_dir,
        shell,
        editor: get_default_editor().unwrap_or_else(|| "Unknown".to_string()),
        kernel_version: uname_data.release,
        desk_env: get_desktop_environment(),
        distro: format!("{} ({})", distro, uname_data.machine),
        uptime: get_uptime(
            // We pass to get_uptime the amount obtained with libc::sysinfo
            sys_info.uptime,
        ),
        total_memory: pretty_bytes(sys_info.total_ram as f64),
        used_memory: pretty_bytes((sys_info.total_ram - sys_info.free_ram) as f64),
        monitor_res: resolution,
    }
}

pub fn get_hostname() -> Option<String> {
    let hostname_max = unsafe { sysconf(libc::_SC_HOST_NAME_MAX) } as usize;
    let mut buffer = vec![0_u8; hostname_max + 1]; // +1 to account for the NUL character
    let ret = unsafe { gethostname(buffer.as_mut_ptr() as *mut c_char, buffer.len()) };

    if ret == 0 {
        let end = buffer
            .iter()
            .position(|&b| b == 0)
            .unwrap_or_else(|| buffer.len());

        buffer.resize(end, 0);
        String::from_utf8(buffer).ok()
    } else {
        None
    }
}

pub fn get_distro() -> Option<String> {
    let distro = std::fs::read_to_string("/etc/os-release").ok()?;

    for line in distro.lines().filter(|line| line.len() >= 11) {
        if let "PRETTY_NAME" = &line[..11] {
            return Some(line[13..].trim_matches('"').to_string());
        }
    }

    Some("Linux".to_string())
}

pub fn get_username_home_dir_and_shell() -> Option<(String, String, String)> {
    // Warning: let rustc infer the type of `buf`, as the value of
    // `buf.as_mut_ptr()` below may vary in type depending on the architecture
    // e.g.: *mut i8 on x86-64
    //       *mut u8 on ARMv7
    let mut buf = [0; 2048];
    let mut result = ptr::null_mut();
    let mut passwd: passwd = unsafe { mem::zeroed() };

    let getpwuid_r_code = unsafe {
        getpwuid_r(
            getuid(),
            &mut passwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        )
    };

    if getpwuid_r_code == 0 && !result.is_null() {
        let username = unsafe { char_ptr_to_string(passwd.pw_name) };
        let home_dir = unsafe { char_ptr_to_string(passwd.pw_dir) };
        
        let shell = unsafe { char_ptr_to_string(passwd.pw_shell) };
        // From "/usr/bin/shell" to just "shell"
        let shell = get_base(&shell);

        Some((username, home_dir, shell))
    } else {
        None
    }
}

pub fn get_cpu_model() -> Option<String> {
    let data = fs::read_to_string("/proc/cpuinfo").ok()?;
    for line in data.lines() {
        if line.len() < 11 {
            continue;
        }
        if let "model name" = &line[..10] {
            return Some(line[12..].splitn(2, '@').next().unwrap().trim().to_string());
        };
    }

    None
}

pub fn get_uptime(uptime_in_centiseconds: usize) -> String {
    let periods: SmallVec<[(u64, &str); 8]> = smallvec![
        (60 * 60 * 24 * 365, "year"),
        (60 * 60 * 24 * 30, "month"),
        (60 * 60 * 24, "day"),
        (60 * 60, "hour"),
        (60, "minute"),
        (1, "second"),
    ];

    // Ignore decimal places
    let mut uptime_in_seconds = uptime_in_centiseconds as u64;
    // Final result
    let mut uptime = String::new();

    for (period, period_name) in periods {
        let times = uptime_in_seconds / period;

        if times > 0 {
            // Add space between entries
            if !uptime.is_empty() {
                uptime.push(' ');
            }

            uptime.push_str(&format!("{} ", times));

            // Add the "year" period name
            uptime.push_str(period_name);

            // Fix plural
            if times > 1 {
                uptime.push('s');
            }

            // Update for next
            uptime_in_seconds %= period;
        }
    }
    uptime
}

pub fn get_default_editor() -> Option<String> {
    let def_editor_path = std::env::var_os("EDITOR")?;
    let def_editor_path = def_editor_path.to_string_lossy();

    // Return the editor's executable name, without its path
    Some(
        get_base(&def_editor_path)
    )
}

pub fn get_desktop_environment() -> String {
    std::env::var_os("DESKTOP_SESSION")
        .map(|env| {
            let env = get_base(env.to_str().unwrap()).to_lowercase();
            match env {
                _ if env.contains("gnome")   => "Gnome",
                _ if env.contains("lxde")    => "LXDE",
                _ if env.contains("openbox") => "OpenBox",
                _ if env.contains("i3")      => "i3",
                _ if env.contains("ubuntu")  => "Ubuntu",
                _ if env.contains("plasma")  => "KDE",
                _ if env.contains("mate")    => "MATE",
                _ => env.as_str(),
            }
            .into()
        })
        .unwrap_or_else(|| "Unknown".to_string())
}
