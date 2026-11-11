//! Python PyPI artifact selection and ABI tag evaluation.

/// Evaluates Python wheel ABI tags and selects the best matching artifact for target platform.
pub fn select_pypi_artifact(
    urls: &[serde_json::Value],
    target_platform: Option<&str>,
) -> Option<String> {
    let host_os = std::env::consts::OS;
    let host_arch = std::env::consts::ARCH;

    let (target_os, target_arch) = if let Some(target) = target_platform {
        let parts: Vec<&str> = target.split('-').collect();
        match (parts.first(), parts.get(1)) {
            (Some(&os), Some(&arch)) => (os, arch),
            _ => (host_os, host_arch),
        }
    } else {
        (host_os, host_arch)
    };

    let is_macos = target_os == "macos" || target_os == "darwin";
    let is_linux = target_os == "linux";
    let is_windows = target_os == "windows" || target_os == "win32";

    let is_arm64 = target_arch == "arm64" || target_arch == "aarch64";
    let is_x86_64 = target_arch == "x86_64" || target_arch == "x64";

    let mut best_score: i32 = -1;
    let mut best_url: Option<String> = None;

    for entry in urls {
        let url_str = match entry.get("url").and_then(|u| u.as_str()) {
            Some(u) => u,
            None => continue,
        };
        let filename = entry
            .get("filename")
            .and_then(|f| f.as_str())
            .unwrap_or("")
            .to_lowercase();

        // Python version bonus for hermetic toolchain compatibility (Python 3.11 default in DMZ)
        let py_bonus: i32 = if filename.contains("cp311") {
            3
        } else if filename.contains("cp312") {
            2
        } else if filename.contains("cp310") || filename.contains("cp313") {
            1
        } else {
            0
        };

        let score: i32 = if filename.ends_with(".whl") {
            if filename.contains("-none-any.whl")
                || filename.contains("py2.py3-none-any")
                || filename.contains("py3-none-any")
            {
                80 // Pure Python universal wheel
            } else if is_macos && (filename.contains("macosx") || filename.contains("darwin")) {
                if (is_arm64 && filename.contains("arm64"))
                    || (is_x86_64 && filename.contains("x86_64"))
                {
                    100 + py_bonus // Exact architecture match
                } else if filename.contains("universal2") {
                    95 + py_bonus // macOS universal binary
                } else {
                    15 + py_bonus // macOS alternative architecture (e.g. x86_64 Rosetta)
                }
            } else if is_linux
                && (filename.contains("manylinux")
                    || filename.contains("musllinux")
                    || filename.contains("linux"))
            {
                if (is_x86_64 && (filename.contains("x86_64") || filename.contains("amd64")))
                    || (is_arm64 && filename.contains("aarch64"))
                {
                    100 + py_bonus // Exact Linux architecture match
                } else {
                    15 + py_bonus // Linux alternative architecture
                }
            } else if is_windows
                && (filename.contains("win_amd64")
                    || filename.contains("win32")
                    || filename.contains("windows"))
            {
                if (is_x86_64 && (filename.contains("amd64") || filename.contains("win_amd64")))
                    || (is_arm64 && filename.contains("arm64"))
                {
                    100 + py_bonus // Exact Windows architecture match
                } else {
                    15 + py_bonus // Windows alternative architecture
                }
            } else {
                // Wheel built for a different OS (e.g. Linux/Windows binary wheel for cross-platform/air-gap export)
                if filename.contains("manylinux")
                    || filename.contains("musllinux")
                    || filename.contains("linux")
                {
                    if (is_x86_64 && (filename.contains("x86_64") || filename.contains("amd64")))
                        || (is_arm64 && filename.contains("aarch64"))
                    {
                        40 + py_bonus // Target/host architecture matching Linux wheel
                    } else {
                        35 + py_bonus // Alternative architecture Linux wheel
                    }
                } else if filename.contains("win_amd64")
                    || filename.contains("win_arm64")
                    || filename.contains("win32")
                {
                    if (is_x86_64 && (filename.contains("amd64") || filename.contains("win_amd64")))
                        || (is_arm64 && filename.contains("arm64"))
                    {
                        30 + py_bonus // Target/host architecture matching Windows wheel
                    } else {
                        25 + py_bonus // Alternative architecture Windows wheel
                    }
                } else {
                    20 + py_bonus // Any other valid platform wheel
                }
            }
        } else if filename.ends_with(".tar.gz") || filename.ends_with(".zip") {
            50 // Source distribution fallback (buildable across platforms)
        } else {
            10 // Other distribution archive
        };

        if score > best_score {
            best_score = score;
            best_url = Some(url_str.to_string());
        }
    }

    best_url
}
