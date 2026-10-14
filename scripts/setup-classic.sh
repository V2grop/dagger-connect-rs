#!/usr/bin/env bash
# V2grop menu inspired by the original layout; uses the independent Rust core.
set -euo pipefail
export DAGGER_MENU_STYLE=classic
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
source "$script_dir/setup.sh"
cyan='' bold='' reset=''
if [[ -t 1 && ${TERM:-dumb} != dumb && -z ${NO_COLOR+x} ]]; then
  cyan=$'\033[0;36m'; bold=$'\033[1m'; reset=$'\033[0m'
fi
install_endpoint() {
  local selected=$1
  echo 'مرحلهٔ 1 از 2: تنظیم سادهٔ ایران / خارج'
  configure "$selected" guided
  echo 'سرور مقابل هم باید تنظیم شود. فقط کلید عمومی بین دو سرور جابه‌جا می‌شود.'
  yes_no "مرحلهٔ 2 از 2: سرویس همین سرور را اکنون فعال کنیم؟ / Step 2/2: start $selected?" yes
  if [[ $REPLY == yes ]]; then
    install_service "$selected"
  else
    echo 'فقط ذخیره شد؛ سرویس راه‌اندازی نشد. / CONFIGURATION ONLY: NOT started.'
    echo 'برای فعال‌کردن تنظیم ذخیره‌شده، گزینهٔ 14 را بزنید.'
  fi
}
service_control() {
  role_prompt || return
  local selected=$role action
  printf '\n  1) Restart\n  2) Stop\n  3) Start\n  4) Status\n  0) Back\n'
  ask 'Choice' 0
  case $REPLY in 1) action=restart ;; 2) action=stop ;; 3) action=start ;; 4) action=status ;; 0) return ;; *) echo 'Invalid choice.'; return ;; esac
  systemctl_cmd "$action" "dagger-rs-$selected.service"
}
edit_config() {
  role_prompt || return
  local file=$config_dir/$role.json temporary cleanup editor=${EDITOR:-}
  [[ -f $file && ! -L $file ]] || { echo 'A regular role config is required.'; return 1; }
  if [[ -z $editor ]]; then
    for editor in nano vi; do command -v "$editor" >/dev/null && break; done
  fi
  command -v "$editor" >/dev/null || { echo 'Install nano or set EDITOR to an editor executable.'; return 1; }
  temporary=$(mktemp "$config_dir/.edit.XXXXXXXX.json")
  printf -v cleanup 'rm -f -- %q' "$temporary"
  trap "$cleanup" EXIT
  cp -- "$file" "$temporary"
  "$editor" "$temporary"
  "$binary" -c "$temporary" --check || { echo 'Invalid configuration; existing file preserved.'; return 1; }
  ask 'Save validated changes? (yes/no)' no
  if [[ $REPLY == yes ]]; then
    cp -p -- "$file" "$file.backup.$(date +%s)"
    chmod --reference="$file" "$temporary"
    if (( EUID == 0 )); then chown --reference="$file" "$temporary"; fi
    mv -fT -- "$temporary" "$file"
    ask 'Restart this service to apply changes? (yes/no)' no
    [[ $REPLY != yes ]] || systemctl_cmd restart "dagger-rs-$role.service"
  fi
}
live_logs() {
  role_prompt || return
  echo 'Press Ctrl+C to stop following logs.'
  local -a args=(-u "dagger-rs-$role.service" -n 50 -f)
  if (( EUID != 0 )); then args=(--user "${args[@]}"); fi
  # Ignore SIGINT only in the menu shell while the foreground journalctl exits.
  trap ':' INT
  journalctl "${args[@]}" || true
  trap - INT
}
remove_service() {
  role_prompt || return
  local units=/etc/systemd/system
  if (( EUID != 0 )); then units=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user; fi
  ask "Remove dagger-rs-$role.service? Configs and keys will be kept. Type yes" no
  [[ $REPLY == yes ]] || return 0
  systemctl_cmd disable --now "dagger-rs-$role.service" || return
  rm -f -- "$units/dagger-rs-$role.service"
  systemctl_cmd daemon-reload
  echo 'Service removed. Configuration and keys retained.'
}
update_core() {
  [[ ${DAGGER_PREFIX:-/usr/local} == /usr/local && $config_dir == /etc/dagger-rs && $EUID == 0 ]] || {
    echo 'For custom/user installations, install a release bundle with your existing --prefix and --config-dir.'; return 1;
  }
  ask 'Download and install the latest V2grop release? (yes/no)' no
  [[ $REPLY == yes ]] || return 0
  exec bash "$script_dir/install-release.sh" --menu classic
}
show_menu() {
  printf '\n  %s%sDagger Rust Installer%s — V2grop\n' "$cyan" "$bold" "$reset"
  printf '  Independent Rust core | rewrite by ir_spoof\n  Config: %s\n\n' "$config_dir"
  printf '  %sInstall%s\n    1) تنظیم سادهٔ سرور ایران / IRAN\n    2) تنظیم سادهٔ سرور خارج / ABROAD\n\n' "$bold" "$reset"
  printf '  %sManage%s\n    3) وضعیت سرویس همین سرور / Status\n    4) شروع، توقف یا ری‌استارت / Control\n    5) ویرایش تخصصی فایل تنظیم / Advanced edit\n\n' "$bold" "$reset"
  printf '  %sLogs%s\n    6) خطاها و گزارش اخیر / Logs\n    7) گزارش زنده / Live logs\n\n' "$bold" "$reset"
  printf '  %sDiagnose%s\n   11) تست تخصصی شبکه / Advanced network test\n   13) عیب‌یابی تونل و سرویس / Diagnose\n\n' "$bold" "$reset"
  printf '  %sOther%s\n    8) حذف سرویس همین سرور / Remove\n    9) نسخهٔ برنامه / Version\n   10) دریافت نسخهٔ جدید / Update\n   12) تنظیمات پیشرفته: TLS / TUN / SOCKS / سایر روش‌ها\n   14) فعال‌کردن تنظیم ذخیره‌شده / Start saved config\n   15) نمایش کلید عمومی همین سرور / Public key\n    0) خروج / Exit\n\n' "$bold" "$reset"
}
if [[ ${1:-} == --help ]]; then echo 'Usage: dagger-setup-classic. Shares keys/configs/services with dagger-setup.'; exit 0; fi
while true; do
  show_menu
  if ! ask 'شمارهٔ گزینه / Choice' 0; then exit 0; fi
  # run_action preserves errexit inside the action and keeps this menu usable.
  case $REPLY in
    1) run_action install_endpoint server ;;
    2) run_action install_endpoint client ;;
    3) run_action service_action status ;;
    4) run_action service_control ;;
    5) run_action edit_config ;;
    6) run_action service_action logs ;;
    7) run_action live_logs ;;
    8) run_action remove_service ;;
    9) "$binary" --version ;;
    10) run_action update_core ;;
    11) run_action linktest_menu ;;
    12) DAGGER_MENU_STYLE=original bash "$script_dir/setup.sh" ;;
    13) run_action diagnose_menu ;;
    14) run_action install_service ;;
    15) run_action key_menu ;;
    0) exit 0 ;;
    *) echo 'Choose a listed option.' ;;
  esac
  read -r -p 'برای برگشت به منو Enter بزنید / Enter: ' _ || exit 0
done
