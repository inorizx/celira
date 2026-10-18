//! 一次活动的**文本创建授权**：宿主显式给出的、范围有限的一份许可。
//!
//! 它回答三个问题，而且只回答这三个：**谁**（哪个 Agent）、**在哪次活动里**（哪个 Run）、
//! **在哪个目录里**可以创建新的文本文件。
//!
//! 边界（本版刻意做小的部分）：
//!
//! - 它**不是通用授权平台**：没有持久授权、没有动态撤销、没有审批流、没有审计日志，也不跨
//!   活动复用。授权随这一次装配进出的对象存活，活动结束即消失；"不能撤销"的窗口就是这次
//!   活动的全过程；
//! - 它**只描述目录与身份**，不做任何文件系统动作：目录是否存在、是不是链接、是否真的落在
//!   授权资料根里，全部由文件组件在**装配期**校验（拒绝启动，而不是静默修正）；
//! - 它**不扩大读权限**：读本身免写效应门，创建授权只增加"能在这一个目录里新建文本"这一件事；
//! - 它**不是安全沙盒**：真正的边界仍是目录、身份与能力限制，以及"受信目录"这个前提。
//!
//! 模型既不能创建也不能扩大授权：授权只能由宿主给出，而模型能给的只有动作参数里的文件名。

use crate::foundation::{AgentId, RunId};

/// 宿主给出的创建授权：绑定身份、活动与目录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationGrant {
    agent: AgentId,
    run: RunId,
    /// **单层**目录名（相对于授权资料根），不是路径。
    directory: String,
}

/// 授权请求本身不合法（还谈不上有没有权限，是这份请求写错了）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantError {
    /// 目录名为空。
    Empty,
    /// 不是单层目录名：含分隔符、盘符、冒号、`.`、`..` 之类都不是"一个目录名"。
    NotSingleDirectoryName,
}

impl GrantError {
    /// 稳定分类标记。
    pub fn as_str(self) -> &'static str {
        match self {
            GrantError::Empty => "empty-directory",
            GrantError::NotSingleDirectoryName => "not-single-directory-name",
        }
    }
}

impl CreationGrant {
    /// 用身份、活动与一个**单层目录名**建立授权请求。
    ///
    /// 这里只做形状校验（不碰文件系统）：一个目录名就是一个目录名，写成一个路径就不是它。
    pub fn new(
        agent: AgentId,
        run: RunId,
        directory: impl Into<String>,
    ) -> Result<Self, GrantError> {
        let directory = directory.into();
        if directory.is_empty() {
            return Err(GrantError::Empty);
        }
        let bad = directory == "."
            || directory == ".."
            || directory.contains('/')
            || directory.contains('\\')
            || directory.contains(':');
        if bad {
            return Err(GrantError::NotSingleDirectoryName);
        }
        Ok(Self {
            agent,
            run,
            directory,
        })
    }

    /// 这份授权属于哪个 Agent。
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// 这份授权只在哪一次活动里有效。
    pub fn run(&self) -> &RunId {
        &self.run
    }

    /// 授权创建的目录名（相对于授权资料根的单层目录）。
    pub fn directory(&self) -> &str {
        &self.directory
    }
}
