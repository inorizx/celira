#!/bin/sh
# Celira 住户身份安装：**建一个专用系统用户，把住户的家放进它名下**。
#
# 为什么要有这一件：`terminal=sandbox` 的 identity 后端不开命名空间，它的写边界就是
# **uid 的 DAC**——"系统写不了、主人的东西写不了、自己的家写得了"。这三条只有在
# 住户**不是**主人那个 uid 时才成立。见 docs/runtime/terminal/SANDBOX_IDENTITY_DECISIONS.md §6。
#
# 三个动作，都幂等：
#   --check   只读：把这个家现在的身份处境打出来（默认动作，不改任何东西）
#   --apply   建用户 / 建目录（要 root）；已存在就只补齐，不覆盖
#   --remove  撤销（要 root）；家里还有住户目录时**拒绝**，除非 --force
#
# 用法：
#   sudo ./celira-identity-setup.sh --check
#   sudo ./celira-identity-setup.sh --apply
#   sudo ./celira-identity-setup.sh --apply --user celira-test --home /tmp/celira-test-home
#
# 它**不做**的事（写在明处）：不装 systemd 单元、不改 shell 配置、不动主人的家目录权限、
# 不 chown 任何已有目录。那些是部署方（scripts/home 的安装动作）的事。

set -eu

USER_NAME=celira
HOME_DIR=/var/lib/celira
ACTION=check
FORCE=no

usage() {
	cat <<'EOF'
用法: celira-identity-setup.sh [--check|--apply|--remove] [--user <名>] [--home <目录>] [--force]

  --check    只读，打印身份处境（默认）
  --apply    建系统用户与目录（要 root；幂等）
  --remove   撤销（要 root；有住户目录时拒绝，除非 --force）
  --user     系统用户名（缺省 celira）
  --home     这个用户的家（缺省 /var/lib/celira）
  --force    --remove 时连住户目录一起删（**会删数据**，只给测试用）
EOF
}

while [ $# -gt 0 ]; do
	case "$1" in
	--check) ACTION=check ;;
	--apply) ACTION=apply ;;
	--remove) ACTION=remove ;;
	--force) FORCE=yes ;;
	--user)
		shift
		USER_NAME=${1:?--user 需要一个名字}
		;;
	--home)
		shift
		HOME_DIR=${1:?--home 需要一个目录}
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		echo "未知参数: $1" >&2
		usage >&2
		exit 2
		;;
	esac
	shift
done

fail() {
	echo "FAIL: $*" >&2
	FAILED=1
}

need_root() {
	if [ "$(id -u)" -ne 0 ]; then
		echo "要以 root 跑（$ACTION 会改系统状态）：sudo $0 $*" >&2
		exit 1
	fi
}

user_exists() {
	getent passwd "$USER_NAME" >/dev/null 2>&1
}

# 目录的读数：owner / 模式（不存在时打印 "-"）。
dir_reading() {
	if [ -d "$1" ]; then
		stat -c '%U:%G %a' "$1"
	else
		echo '-'
	fi
}

case "$ACTION" in
check)
	FAILED=0
	echo "== Celira 住户身份 =="
	if user_exists; then
		uid=$(id -u "$USER_NAME")
		echo "user      = $USER_NAME (uid $uid)"
		echo "home      = $(dir_reading "$HOME_DIR")  $HOME_DIR"
		if [ -d "$HOME_DIR/agents" ]; then
			echo "agents    = $(dir_reading "$HOME_DIR/agents")  $HOME_DIR/agents"
			echo "residents = $(find "$HOME_DIR/agents" -mindepth 1 -maxdepth 1 -type d | wc -l)"
		else
			echo "agents    = -"
		fi
		# 判据：家的 owner 必须是 root（住户 uid 不能改家的结构），否则它能在家里造兄弟目录。
		owner=$(stat -c '%U' "$HOME_DIR" 2>/dev/null || echo '-')
		if [ "$owner" != root ]; then
			fail "$HOME_DIR 的 owner 是 $owner，不是 root：住户能在家里造/删同级的家"
		fi
		if [ "$uid" -eq 0 ]; then
			fail "$USER_NAME 的 uid 是 0：这一档的写边界整个不成立"
		fi
	else
		echo "user      = 未建（$USER_NAME）"
		fail "专用身份不存在：identity 后端兑现不了「写不了主人的东西」"
	fi
	if [ "$FAILED" -ne 0 ]; then
		exit 1
	fi
	echo "== OK =="
	;;

apply)
	need_root
	if user_exists; then
		echo "user $USER_NAME 已存在（uid $(id -u "$USER_NAME")），不动它"
	else
		useradd --system --home-dir "$HOME_DIR" --shell /usr/sbin/nologin \
			--user-group "$USER_NAME"
		echo "user $USER_NAME 已建（uid $(id -u "$USER_NAME")）"
	fi
	# 家的 owner 是 root：住户写得了自己那一格，改不了家的结构（造不出兄弟目录）。
	install -d -m 0755 -o root -g root "$HOME_DIR"
	install -d -m 0755 -o root -g root "$HOME_DIR/agents"
	echo "布局就绪：$HOME_DIR（root 0755） / $HOME_DIR/agents（root 0755）"
	echo
	echo "接下来（**每个住户一格**，owner 是住户身份）："
	echo "  install -d -m 0700 -o $USER_NAME -g $USER_NAME $HOME_DIR/agents/<agent-id>"
	echo "然后用这个身份启动宿主（HOME=$HOME_DIR），再开 --terminal sandbox。"
	;;

remove)
	need_root
	if [ -d "$HOME_DIR/agents" ] && [ -n "$(find "$HOME_DIR/agents" -mindepth 1 -maxdepth 1 2>/dev/null)" ]; then
		if [ "$FORCE" != yes ]; then
			echo "拒绝：$HOME_DIR/agents 下还有住户目录（那是数据）。确实要删就加 --force" >&2
			exit 1
		fi
		echo "警告：--force ⇒ 连住户目录一起删" >&2
		rm -rf "$HOME_DIR/agents"
	fi
	if user_exists; then
		userdel "$USER_NAME" 2>/dev/null || true
		echo "user $USER_NAME 已删"
	fi
	rmdir "$HOME_DIR/agents" 2>/dev/null || true
	rmdir "$HOME_DIR" 2>/dev/null || true
	echo "清理完成（非空目录不删）"
	;;
esac
