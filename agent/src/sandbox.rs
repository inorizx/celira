//! **沙盒路径机制**：一个相对路径在某个根之下意味着什么——这件事只有这一份实现。
//!
//! 为什么它不在任何一个 App 里：`files`（主体的资料与写作）、`file_view`（派生只读视图）与
//! `im`（把主体的产出递出去）都要同一条规则，而 **App 之间不得有实现依赖**——结构性守卫
//! `tests/minimal_agent.rs` 的 `boundary_apis_stay_in_their_own_layer` 机械检查这一条，
//! TASK_65 期间它正是拦下了"im 直接引用 file_app"的写法。规则因此落在中间层：谁要用谁来取，
//! 机制只有一份、没有第二份权威——`file_app` 那两个同名函数只是**转发**。
//!
//! 它判的是**形状与存在性**、给的是**字节**：不判"这是不是文本"（那是调用方按自己的白名单
//! 做的事），也不判"这个文件该不该被读"（那是权限，在世界那一侧判）。
//!
//! 安全等级与 `files` 同档：沙盒是**受信目录**（不允许不可信进程并发改写）。逐段
//! `symlink_metadata` 检查与之后真正的打开之间存在理论上的替换窗口，本模块**不构成**
//! OS 级安全沙盒，也不承诺抵御恶意并发路径替换。

use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// 沙盒操作自己的稳定失败分类。
///
/// 它**不是**某个 App 的词表：每个调用方把它映射成自己的那套（`files` 有 `FailureClass`，
/// `im` 有 `ImFailure`）。这样"机制"与"语言"各自只有一份，谁都不必认识谁的措辞。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxError {
    /// 相对路径本身不合法：绝对路径、盘符、UNC、设备路径、`:`（Windows ADS）、`.`、`..`、空路径。
    InvalidPath,
    /// 目标不存在。
    NotFound,
    /// 目标类型不对：要的是**一个文件**，拿到的不是普通文件。
    WrongKind,
    /// 访问链上出现符号链接、junction 或其他 reparse point。
    Link,
    /// 读取被拒绝（权限）。
    Denied,
    /// 超过调用方给的上限：明确拒绝，**不**截断。
    TooLarge,
    /// 这段内容读不出来（存储层报错：损坏、越界、未知尾部）。
    Unreadable,
    /// 其他 I/O 失败。
    Io,
}

impl SandboxError {
    /// 稳定分类标记。
    pub fn as_str(self) -> &'static str {
        match self {
            SandboxError::InvalidPath => "invalid-path",
            SandboxError::NotFound => "not-found",
            SandboxError::WrongKind => "wrong-kind",
            SandboxError::Link => "link",
            SandboxError::Denied => "denied",
            SandboxError::TooLarge => "too-large",
            SandboxError::Unreadable => "unreadable",
            SandboxError::Io => "io",
        }
    }
}

/// 把一个**沙盒相对路径**拆成普通名字组件（第一遍：只看形状，不碰文件系统）。
///
/// 空串返回空组件表（调用方自行决定是否允许，空串表示沙盒根）。拒绝绝对路径、`\` 或 `/`
/// 开头、UNC、设备路径、任何含 `:` 的写法（含 Windows ADS）、`.`、`..`、根与盘符前缀。
pub fn relative_names(relative: &str) -> Result<Vec<String>, SandboxError> {
    if relative.is_empty() {
        return Ok(Vec::new());
    }
    if relative.contains(':') {
        return Err(SandboxError::InvalidPath);
    }
    if relative.starts_with('\\') || relative.starts_with('/') {
        return Err(SandboxError::InvalidPath);
    }
    let raw = Path::new(relative);
    if raw.is_absolute() {
        return Err(SandboxError::InvalidPath);
    }

    let mut names: Vec<String> = Vec::new();
    for component in raw.components() {
        match component {
            Component::Normal(name) => names.push(name.to_string_lossy().into_owned()),
            _ => return Err(SandboxError::InvalidPath),
        }
    }
    if names.is_empty() {
        return Err(SandboxError::InvalidPath);
    }
    Ok(names)
}

/// **第二遍**：在一个沙盒根下逐段落实一个已经通过形状校验的相对路径。
///
/// 空组件表返回根本身（`files_list` 的沙盒根就是它的合法目标）；非空时逐段确认存在、拒绝
/// 符号链接 / junction / 其他 reparse point，最后按**组件**比较确认结果仍在根之内
/// （`Path::starts_with` 按组件比较，不是字符串前缀比较）。
pub fn resolve_within(root: &Path, names: &[String]) -> Result<PathBuf, SandboxError> {
    let mut current = root.to_path_buf();
    for name in names {
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| io_error_class(&error))?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(SandboxError::Link);
        }
    }
    if !current.starts_with(root) {
        return Err(SandboxError::InvalidPath);
    }
    Ok(current)
}

/// 在一个沙盒根下读**一个普通文件**的字节（上限 `max` 字节；超限响亮拒绝，**不截断**）。
///
/// 它给的是**字节**而不是文本：调用方（im app 递文件）要的是原样递出去；判"这是不是文本类"
/// 是调用方按自己的白名单做的**另一件事**。这里的边界只有三条，都是目录与文件系统的边界——
/// 路径形状（[`relative_names`]）、链上每一段（[`resolve_within`]）、目标必须是普通文件且
/// 不超过 `max`。
///
/// 超限判**两次**（先看 `symlink_metadata` 的长度，再用 `max + 1` 的限额读）：只判一次够不上
/// "响亮"——检查与打开之间文件可能长大，那一次增长不能被"看起来读完了"吞掉。
pub fn read_sandbox_file(root: &Path, relative: &str, max: u64) -> Result<Vec<u8>, SandboxError> {
    let names = relative_names(relative)?;
    if names.is_empty() {
        return Err(SandboxError::InvalidPath);
    }
    let path = resolve_within(root, &names)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| io_error_class(&error))?;
    if !metadata.is_file() {
        return Err(SandboxError::WrongKind);
    }
    if metadata.len() > max {
        return Err(SandboxError::TooLarge);
    }
    let file = File::open(&path).map_err(|error| io_error_class(&error))?;
    let mut bytes = Vec::new();
    let mut limited = file.take(max + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| io_error_class(&error))?;
    if bytes.len() as u64 > max {
        return Err(SandboxError::TooLarge);
    }
    Ok(bytes)
}

/// 在一个沙盒根下看**一个普通文件**的名字与字节数（**不读内容**）。
///
/// 与 [`read_sandbox_file`] 同一套路径规则、同一条"必须是普通文件"的要求；它只回答两件事：
/// 最后一段叫什么、有多大。调用方（`im` 的复核）要用这两样做比对——服务端不返回 hash，
/// "同一份文件"在那一层只能用可观察的元数据说。
pub fn stat_sandbox_file(root: &Path, relative: &str) -> Result<(String, u64), SandboxError> {
    let names = relative_names(relative)?;
    if names.is_empty() {
        return Err(SandboxError::InvalidPath);
    }
    let path = resolve_within(root, &names)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| io_error_class(&error))?;
    if !metadata.is_file() {
        return Err(SandboxError::WrongKind);
    }
    let name = names.last().cloned().unwrap_or_default();
    Ok((name, metadata.len()))
}

/// **机器档**：一条**绝对路径**在这台机器上指向哪儿 —— 卷根 ＋ 卷根之下的名字段。
///
/// 它只服务一种装配：**主人把整台机器当文件面**给了这一支（`files` 的"全开"档）。
/// 相对路径那一档（[`relative_names`]）一个字没动 —— 两档的形状**互不接受对方的写法**：
/// 机器档拒绝相对路径（"相对于什么"在这里没有唯一答案），家档拒绝绝对路径（那就出根了）。
///
/// ⚠ **进度（2026-10-20）**：这一对是那一刀的**第一层** —— 判据与逐段检查在，**装配还没接**
/// （`file_app` 的全开档、`--files` 旗标、`agent.conf` 那一行、页面上那一格都是下一步）。
/// ⇒ 它今天**不是**一个已经生效的能力，别照着它去假设"主体已经能碰整台机器"。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachinePath {
    root: PathBuf,
    names: Vec<String>,
}

impl MachinePath {
    /// 命中的卷根（`C:\`、`D:\`；Linux 上是 `/`）。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 卷根之下的名字段（已经过形状校验）。
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// 卷根本身（`files_list` 的合法目标）。
    pub fn is_volume_root(&self) -> bool {
        self.names.is_empty()
    }
}

/// **机器档的第一遍**：一条绝对路径拆成"卷根 ＋ 名字段"（只看形状，不碰文件系统）。
///
/// 拒的东西比家档多一条，因为这里没有"根"把逃逸兜住：
/// - UNC（`\\server\share`）、设备命名空间（`\\?\`、`\\.\`）、盘符相对（`C:foo`）；
/// - 任何**第二个** `:`（Windows ADS `file.txt:stream`，也能写成别的盘符）；
/// - `.`、`..`、空路径；
/// - **Windows 上**：名字以点或空格结尾（Win32 会把它们悄悄剪掉 ⇒ 同一个文件两个写法，
///   形状校验就白判了）、保留设备名（`CON` / `NUL` / `COM1`… 打开的是设备，不是文件）。
///
/// ⚠ 与家档同一条等级声明：这是"形状"判据，不是安全沙盒（见本文件头）。
pub fn machine_path(absolute: &str) -> Result<MachinePath, SandboxError> {
    let (root, names) = split_absolute(absolute)?;
    Ok(MachinePath { root, names })
}

/// **机器档的第二遍**：逐段落实一条已经通过形状校验的绝对路径。
///
/// 与 [`resolve_within`] 同一套逐段检查（符号链接 / junction / reparse point 一律拒），
/// 区别只有基准：那里是一个沙盒根，这里是**这条路径自己点名的那个卷根**——所以"在不在根里"
/// 由卷本身兜住，不需要（也无法）拿一个更外层的根去比。
///
/// 卷不在（`Q:\`）⇒ `NotFound`；卷在而中途某段不在 ⇒ `NotFound`；最后按组件比一遍
/// （`Path::starts_with`，不是字符串前缀）。
pub fn resolve_machine(target: &MachinePath) -> Result<PathBuf, SandboxError> {
    let metadata = fs::symlink_metadata(&target.root).map_err(|error| io_error_class(&error))?;
    if !metadata.is_dir() {
        return Err(SandboxError::WrongKind);
    }
    let mut current = target.root.clone();
    for name in &target.names {
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| io_error_class(&error))?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(SandboxError::Link);
        }
    }
    if !current.starts_with(&target.root) {
        return Err(SandboxError::InvalidPath);
    }
    Ok(current)
}

/// 绝对路径 → `(卷根, 名字段)`。平台差异**只在这里**（家档那条"反斜杠不是 Linux 的分隔符"
/// 的教训：形状判据不许把某一平台的写法当成跨平台判据，见本文件头）。
#[cfg(windows)]
fn split_absolute(absolute: &str) -> Result<(PathBuf, Vec<String>), SandboxError> {
    let bytes = absolute.as_bytes();
    // `X:\` 或 `X:/`：盘符 ＋ 冒号 ＋ 分隔符，缺一不可（`C:foo` 是"当前盘上的相对路径"）。
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return Err(SandboxError::InvalidPath);
    }
    if bytes[2] != b'\\' && bytes[2] != b'/' {
        return Err(SandboxError::InvalidPath);
    }
    let root = PathBuf::from(format!("{}:\\", (bytes[0] as char).to_ascii_uppercase()));
    // 第二个 `:` ⇒ ADS 或另一个盘符；UNC / `\\?\` / `\\.\` 在第一步就不成立（不是 `X:\` 形状）。
    if absolute[2..].contains(':') {
        return Err(SandboxError::InvalidPath);
    }
    let names = split_names(&absolute[3..])?;
    Ok((root, names))
}

#[cfg(not(windows))]
fn split_absolute(absolute: &str) -> Result<(PathBuf, Vec<String>), SandboxError> {
    if !absolute.starts_with('/') {
        return Err(SandboxError::InvalidPath);
    }
    Ok((PathBuf::from("/"), split_names(&absolute[1..])?))
}

/// 名字段：跳过连续分隔符；空段不产生名字；`.` / `..` 一律拒（机器档没有"上一层"这回事）。
fn split_names(rest: &str) -> Result<Vec<String>, SandboxError> {
    let mut names: Vec<String> = Vec::new();
    for part in rest.split(['\\', '/']) {
        if part.is_empty() {
            continue;
        }
        if part == "." || part == ".." {
            return Err(SandboxError::InvalidPath);
        }
        #[cfg(windows)]
        {
            if part.ends_with('.') || part.ends_with(' ') {
                return Err(SandboxError::InvalidPath);
            }
            if is_reserved_device_name(part) {
                return Err(SandboxError::InvalidPath);
            }
        }
        names.push(part.to_string());
    }
    Ok(names)
}

/// Windows 保留设备名（`CON` / `NUL` / `COM1`…）：它们打开的是设备，不是文件。
/// 带扩展名也算（`CON.txt` 仍是设备）。家档不判这一条（那里有根兜着，且不是本刀的范围）。
#[cfg(windows)]
fn is_reserved_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// 是否是符号链接 / junction / 其他 reparse point。
///
/// Windows 上 junction 也是 reparse point；只看 `is_symlink()` 不足以覆盖全部情况，
/// 因此这里直接看 `FILE_ATTRIBUTE_REPARSE_POINT`。
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

/// I/O 错误 → 本模块的稳定分类。
fn io_error_class(error: &std::io::Error) -> SandboxError {
    match error.kind() {
        std::io::ErrorKind::NotFound => SandboxError::NotFound,
        std::io::ErrorKind::PermissionDenied => SandboxError::Denied,
        std::io::ErrorKind::InvalidData => SandboxError::Unreadable,
        _ => SandboxError::Io,
    }
}
