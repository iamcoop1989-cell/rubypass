// macOS and Linux privileged helper management.
// Installs a fixed helper script + sudoers NOPASSWD rule so subsequent
// route operations never prompt for a password.

use std::io::Write;
use std::process::{Command, Stdio};

const HELPER_PATH: &str = "/usr/local/lib/rubypass/apply.sh";
const SUDOERS_PATH: &str = "/etc/sudoers.d/rubypass";

// ── macOS ─────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
const HELPER_VERSION: &str = "v2";

// Helper script content — validates all inputs before passing to route(8).
#[cfg(target_os = "macos")]
const HELPER_SCRIPT: &str = "#!/bin/sh
# rubypass-helper v2
ACTION=$1
GATEWAY=$2
case \"$ACTION\" in
    add|delete|change) ;;
    *) exit 1 ;;
esac
printf '%s' \"$GATEWAY\" | grep -qE '^[0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+$' || exit 1
while IFS= read -r CIDR; do
    printf '%s' \"$CIDR\" | grep -qE '^[0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+/[0-9]+$' || continue
    if [ \"$ACTION\" = \"change\" ]; then
        route change -net \"$CIDR\" \"$GATEWAY\" 2>/dev/null || route add -net \"$CIDR\" \"$GATEWAY\" 2>/dev/null || true
    else
        route \"$ACTION\" -net \"$CIDR\" \"$GATEWAY\" 2>/dev/null || true
    fi
done
";

// ── Linux ─────────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
const HELPER_VERSION: &str = "v1-linux";

// Helper script content — validates all inputs before passing to ip-route(8).
#[cfg(target_os = "linux")]
const HELPER_SCRIPT: &str = "#!/bin/sh
# rubypass-helper v1-linux
ACTION=$1
GATEWAY=$2
case \"$ACTION\" in
    add|delete|change) ;;
    *) exit 1 ;;
esac
printf '%s' \"$GATEWAY\" | grep -qE '^[0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+$' || exit 1
while IFS= read -r CIDR; do
    printf '%s' \"$CIDR\" | grep -qE '^[0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+/[0-9]+$' || continue
    if [ \"$ACTION\" = \"add\" ]; then
        ip route add \"$CIDR\" via \"$GATEWAY\" 2>/dev/null || true
    elif [ \"$ACTION\" = \"delete\" ]; then
        ip route del \"$CIDR\" via \"$GATEWAY\" 2>/dev/null || true
    else
        ip route change \"$CIDR\" via \"$GATEWAY\" 2>/dev/null || ip route add \"$CIDR\" via \"$GATEWAY\" 2>/dev/null || true
    fi
done
";

// ── shared ────────────────────────────────────────────────────────────────────

pub fn is_installed() -> bool {
    if !std::path::Path::new(HELPER_PATH).exists()
        || !std::path::Path::new(SUDOERS_PATH).exists()
    {
        return false;
    }
    // Check version tag embedded in the script.
    std::fs::read_to_string(HELPER_PATH)
        .map(|s| s.contains(HELPER_VERSION))
        .unwrap_or(false)
}

// ── macOS install ─────────────────────────────────────────────────────────────

/// One-time installation: writes helper + sudoers rule via a single osascript prompt.
#[cfg(target_os = "macos")]
pub fn install() -> Result<(), String> {
    // Write helper script to a temp path first.
    let tmp_helper = "/tmp/rubypass_apply_helper.sh";
    std::fs::write(tmp_helper, HELPER_SCRIPT)
        .map_err(|e| format!("Ошибка записи хелпера: {}", e))?;

    // Build an install shell script that moves and configures everything as root.
    let sudoers_line = format!("%admin ALL=(root) NOPASSWD: {}", HELPER_PATH);
    let install_sh = format!(
        "#!/bin/sh\n\
         mkdir -p /usr/local/lib/rubypass\n\
         cp {tmp} {dst}\n\
         chmod 755 {dst}\n\
         chown root:wheel {dst}\n\
         printf '%s\\n' '{sudoers}' > {sudoers_path}\n\
         chmod 440 {sudoers_path}\n",
        tmp = tmp_helper,
        dst = HELPER_PATH,
        sudoers = sudoers_line,
        sudoers_path = SUDOERS_PATH,
    );

    let tmp_install = "/tmp/rubypass_install.sh";
    std::fs::write(tmp_install, &install_sh)
        .map_err(|e| format!("Ошибка записи скрипта установки: {}", e))?;

    let osa = format!(
        "do shell script \"sh {}\" with administrator privileges",
        tmp_install
    );
    let ok = Command::new("osascript")
        .args(["-e", &osa])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let _ = std::fs::remove_file(tmp_helper);
    let _ = std::fs::remove_file(tmp_install);

    if ok {
        Ok(())
    } else {
        Err("Установка хелпера отменена или завершилась с ошибкой".to_string())
    }
}

// ── Linux install ─────────────────────────────────────────────────────────────

/// Detect which privileged group controls sudo on this distro.
/// Debian/Ubuntu use %sudo; RHEL/Arch/Fedora use %wheel.
#[cfg(target_os = "linux")]
fn detect_sudoers_group() -> &'static str {
    let has_group = |name: &str| {
        Command::new("getent")
            .args(["group", name])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if has_group("sudo") {
        "sudo"
    } else if has_group("wheel") {
        "wheel"
    } else {
        "sudo"
    }
}

/// One-time installation: writes helper + sudoers rule via a single pkexec prompt.
#[cfg(target_os = "linux")]
pub fn install() -> Result<(), String> {
    let tmp_helper = "/tmp/rubypass_apply_helper.sh";
    std::fs::write(tmp_helper, HELPER_SCRIPT)
        .map_err(|e| format!("Ошибка записи хелпера: {}", e))?;

    let group = detect_sudoers_group();
    let sudoers_line = format!("%{} ALL=(root) NOPASSWD: {}", group, HELPER_PATH);
    let install_sh = format!(
        "#!/bin/sh\n\
         mkdir -p /usr/local/lib/rubypass\n\
         cp {tmp} {dst}\n\
         chmod 755 {dst}\n\
         chown root:root {dst}\n\
         printf '%s\\n' '{sudoers}' > {sudoers_path}\n\
         chmod 440 {sudoers_path}\n",
        tmp = tmp_helper,
        dst = HELPER_PATH,
        sudoers = sudoers_line,
        sudoers_path = SUDOERS_PATH,
    );

    let tmp_install = "/tmp/rubypass_install.sh";
    std::fs::write(tmp_install, &install_sh)
        .map_err(|e| format!("Ошибка записи скрипта установки: {}", e))?;

    let ok = Command::new("pkexec")
        .args(["--", "sh", tmp_install])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let _ = std::fs::remove_file(tmp_helper);
    let _ = std::fs::remove_file(tmp_install);

    if ok {
        Ok(())
    } else {
        Err("Установка хелпера отменена или завершилась с ошибкой".to_string())
    }
}

// ── shared run ────────────────────────────────────────────────────────────────

/// Run route operations via the installed helper (no password prompt).
/// Returns number of subnets processed, or 0 on failure.
pub fn run(action: &str, subnets: &[&str], gateway: &str) -> usize {
    let mut child = match Command::new("sudo")
        .args(["-n", HELPER_PATH, action, gateway])
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("sudo helper spawn failed: {}", e);
            return 0;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        for cidr in subnets {
            let _ = writeln!(stdin, "{}", cidr);
        }
    }

    if child.wait().map(|s| s.success()).unwrap_or(false) {
        subnets.len()
    } else {
        0
    }
}
