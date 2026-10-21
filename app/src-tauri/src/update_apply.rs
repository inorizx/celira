//! **把新版本取下来，并换成它** —— 关于页那一颗「下载并更新」的后半段。
//!
//! ## 四步，每一步都能单独失败
//!
//! 1. **下载**到 `%TEMP%\celira-update-<版本>.zip`，落盘后算 sha256；
//! 2. **校字节**：与渠道给的那一串比，不匹配就删掉、不往下走（没有 sha256 也拒绝）；
//! 3. **解压**到**安装目录的同级** `<安装目录>-new-<版本>`（同级是为了后面 rename 同卷、瞬时）；
//! 4. **替换**：旧目录改名 `<安装目录>.old-<时间戳>`（留着，作回滚点），新目录改成原名。
//!
//! ## 为什么替换要交给一个外部进程
//!
//! Windows 不允许覆盖正在运行的 exe。壳自己不能把自己换掉 —— 所以写一段 PowerShell
//! （用 `-EncodedCommand` 传，避开中文路径的编码问题），由它**等壳退出后**做两次 rename，
//! 成功则启动新 exe，失败则把 `.old` 改回来。
//!
//! ## 边界
//!
//! - **不碰家**：更新只换程序目录；家的数据一个字节都不动（`HOME_PORTABILITY.md` 的迁移是另一件事）。
//! - **不在失败后留下半个目录**：解压/校验失败会清掉 payload；rename 失败会把 `.old` 改回去。
//! - **只接受 https**：渠道地址来自 `VERSION` 的 repo/mirror，但地址本身仍要校验协议。
//! - Windows 之外的平台：`tar` 读不了 zip，这里**如实拒绝**（不假装支持）。

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 解压好、验过版本的那一份（下一步 `update_apply` 消费它）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedUpdate {
    pub payload_dir: String,
    pub version: String,
    pub install_dir: String,
}

fn install_dir() -> Result<PathBuf, String> {
    std::env::current_exe()
        .map_err(|error| format!("读不到自己的路径：{error}"))?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "自己的路径没有父目录".to_string())
}

/// **纯函数**：字节摘要对不对（两边都小写后比；空/长度不对一律算不符）。
pub fn digest_matches(actual: &str, expected: &str) -> bool {
    let actual = actual.trim().to_ascii_lowercase();
    let expected = expected.trim().to_ascii_lowercase();
    expected.len() == 64 && actual == expected
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| format!("打不开下载的文件：{error}"))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|error| format!("读下载的文件失败：{error}"))?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// **纯函数**：这一份解压出来的东西，是不是"那个版本的包"。
///
/// 判据只有两条，但都必须有：`VERSION` 里的 `version` 等于期望值；`celira-app.exe` 在场。
/// 少一条就拒绝 —— 一个"看起来像包"的目录被换上去，比不更新更坏。
pub fn payload_is_ok(dir: &Path, version: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(dir.join("VERSION"))
        .map_err(|error| format!("解压出来的东西没有 VERSION：{error}"))?;
    let found = crate::update::channel_of(&text, "version")
        .ok_or_else(|| "VERSION 里没有 version 那一行".to_string())?;
    if found != version {
        return Err(format!("包里的版本是 {found}，不是要更新到的 {version}"));
    }
    if !dir.join("celira-app.exe").is_file() {
        return Err("包解压后没有 celira-app.exe".to_string());
    }
    Ok(())
}

/// PowerShell 的 `-EncodedCommand` 要 **UTF-16LE + base64**。
///
/// 手写而不是加一个 base64 依赖：这段代码只服务一条短命令，依赖树已经够长。
pub fn base64_utf16(text: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = text.encode_utf16().flat_map(|unit| unit.to_le_bytes()).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 63) as usize] as char);
        out.push(TABLE[((triple >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn ps_quote(value: &str) -> String {
    value.replace('\'', "''")
}

/// **纯函数**：等壳退出后执行的替换脚本（可测：只生成文本，不执行）。
///
/// 顺序是判据：**先改名旧目录**（它必须成功，否则什么都不做），**再改名新目录**；
/// 第二步失败就把 `.old` 改回原名。这样最坏的结果是"还是旧版"，不是"两个都没有"。
pub fn apply_script(install: &str, payload: &str, stamp: &str) -> String {
    let install_q = ps_quote(install);
    let payload_q = ps_quote(payload);
    let old_name = format!(
        "{}.old-{stamp}",
        Path::new(install)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "celira-app".to_string())
    );
    let new_name = Path::new(install)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "celira-app".to_string());
    format!(
        "Start-Sleep -Seconds 2\n\
         $install = '{install_q}'\n\
         $payload = '{payload_q}'\n\
         $parent = Split-Path -Parent $install\n\
         $old = Join-Path $parent '{old_name}'\n\
         try {{\n\
           Rename-Item -LiteralPath $install -NewName '{old_name}' -ErrorAction Stop\n\
           Rename-Item -LiteralPath $payload -NewName '{new_name}' -ErrorAction Stop\n\
         }} catch {{\n\
           if ((Test-Path -LiteralPath $old) -and -not (Test-Path -LiteralPath $install)) {{\n\
             Rename-Item -LiteralPath $old -NewName '{new_name}' -ErrorAction SilentlyContinue\n\
           }}\n\
           exit 1\n\
         }}\n\
         Start-Process -FilePath (Join-Path $install 'celira-app.exe')\n"
    )
}

/// 下载、校字节、解压、验版本。**失败不留半个 payload**。
#[tauri::command]
pub async fn update_download(
    version: String,
    url: String,
    sha256: Option<String>,
) -> Result<PreparedUpdate, String> {
    if !url.starts_with("https://") {
        return Err("更新地址不是 https，拒绝下载".to_string());
    }
    let expected = sha256.ok_or_else(|| "渠道没有给出 sha256 —— 不能自动更新".to_string())?;
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("渠道给的 sha256 不是一个 64 位十六进制串".to_string());
    }
    if !cfg!(target_os = "windows") {
        return Err("这个平台的自动更新还没做（解压依赖 Windows 自带的 tar）".to_string());
    }

    let install = install_dir()?;
    let parent = install
        .parent()
        .ok_or_else(|| "安装目录没有父目录".to_string())?
        .to_path_buf();
    let leaf = install
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| "安装目录没有名字".to_string())?;
    let payload = parent.join(format!("{leaf}-new-{version}"));
    let zip = std::env::temp_dir().join(format!("celira-update-{version}.zip"));

    if payload.exists() {
        std::fs::remove_dir_all(&payload).map_err(|error| format!("清不掉旧的临时目录：{error}"))?;
    }

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| format!("HTTP 客户端建不起来：{error}"))?;
    let bytes = http
        .get(&url)
        .header("User-Agent", "celira-app-update")
        .send()
        .await
        .map_err(|error| format!("下载失败：{error}"))?
        .error_for_status()
        .map_err(|error| format!("下载失败：{error}"))?
        .bytes()
        .await
        .map_err(|error| format!("下载中断：{error}"))?;
    std::fs::write(&zip, &bytes).map_err(|error| format!("写不进临时目录：{error}"))?;

    let actual = sha256_file(&zip)?;
    if !digest_matches(&actual, &expected) {
        let _ = std::fs::remove_file(&zip);
        return Err(format!("下载的包 sha256 不符：{actual}"));
    }

    std::fs::create_dir_all(&payload).map_err(|error| format!("建解压目录失败：{error}"))?;
    let status = std::process::Command::new("tar")
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&payload)
        .status()
        .map_err(|error| format!("解压失败（找不到 tar？）：{error}"))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&payload);
        let _ = std::fs::remove_file(&zip);
        return Err(format!("解压失败（tar 退出 {status}）"));
    }
    if let Err(why) = payload_is_ok(&payload, &version) {
        let _ = std::fs::remove_dir_all(&payload);
        let _ = std::fs::remove_file(&zip);
        return Err(why);
    }
    let _ = std::fs::remove_file(&zip);

    Ok(PreparedUpdate {
        payload_dir: payload.display().to_string(),
        version,
        install_dir: install.display().to_string(),
    })
}

/// 写替换脚本、起一个外部 PowerShell、然后壳退出。
///
/// `stamp` 由调用方（前端）给不出来也不该给 —— 这里自己取时间，保证回滚目录名唯一。
#[tauri::command]
pub fn update_apply(app: tauri::AppHandle, payload_dir: String) -> Result<(), String> {
    let install = install_dir()?;
    if !Path::new(&payload_dir).is_dir() {
        return Err("要换上去的那份 payload 不在了".to_string());
    }
    let stamp = format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|each| each.as_secs())
            .unwrap_or(0)
    );
    let script = apply_script(
        &install.display().to_string(),
        &payload_dir,
        &stamp,
    );
    let encoded = base64_utf16(&script);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-EncodedCommand",
                &encoded,
            ])
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("起不来替换进程：{error}"))?;
    }
    #[cfg(not(windows))]
    {
        return Err("这个平台的自动更新还没做".to_string());
    }

    // 让脚本先起来，再退；脚本自己等 2 秒，等的是"这个进程真的退出"。
    std::thread::sleep(Duration::from_millis(300));
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_comparison_is_exact_and_case_insensitive() {
        let hex = "e2b56326702af450dcfba5bd9fbb5a3d6df9caf4cc25bdae0d93d0bbf9954d81";
        assert!(digest_matches(hex, hex));
        assert!(digest_matches(&hex.to_uppercase(), hex));
        assert!(!digest_matches("deadbeef", hex));
        assert!(!digest_matches(hex, ""));
    }

    #[test]
    fn payload_must_carry_the_expected_version_and_the_exe() {
        let dir = std::env::temp_dir().join(format!("celira-payload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        // 没有 VERSION ⇒ 拒绝。
        assert!(payload_is_ok(&dir, "0.1.4").is_err());

        std::fs::write(dir.join("VERSION"), "version = 0.1.4\n").expect("VERSION");
        // 有 VERSION、没 exe ⇒ 拒绝。
        assert!(payload_is_ok(&dir, "0.1.4").is_err());

        std::fs::write(dir.join("celira-app.exe"), b"stub").expect("exe");
        // 版本不符 ⇒ 拒绝。
        assert!(payload_is_ok(&dir, "0.1.5").is_err());
        // 三样都对 ⇒ 通过。
        assert!(payload_is_ok(&dir, "0.1.4").is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_script_renames_old_first_and_has_a_way_back() {
        let script = apply_script(
            r"D:\celira-app",
            r"D:\celira-app-new-0.1.4",
            "1760000000",
        );
        // 先改旧目录，再改新目录 —— 顺序反了就会"两个都没有"。
        let old_at = script.find("Rename-Item -LiteralPath $install").expect("rename old");
        let new_at = script.find("Rename-Item -LiteralPath $payload").expect("rename new");
        assert!(old_at < new_at);
        // 失败时的回退：把 .old 改回去。
        assert!(script.contains("celira-app.old-1760000000"));
        assert!(script.contains("Rename-Item -LiteralPath $old -NewName 'celira-app'"));
        // 最后启动新 exe。
        assert!(script.contains(r"Join-Path $install 'celira-app.exe'"));
    }

    #[test]
    fn ps_quote_escapes_single_quotes() {
        assert_eq!(ps_quote(r"D:\it's"), r"D:\it''s");
    }

    #[test]
    fn base64_utf16_is_what_powershell_expects() {
        // "A" 的 UTF-16LE 是 41 00 ⇒ base64 的 "QQA="（三个字节补一个 0）。
        assert_eq!(base64_utf16("A"), "QQA=");
        // 偶数长度的 ASCII：每个字符两个字节。
        assert_eq!(base64_utf16("AB"), "QQBCAA==");
        // 非 ASCII（中文）也要能编（不 panic、长度按 UTF-16 码元算）。
        assert!(!base64_utf16("更新").is_empty());
    }
}
