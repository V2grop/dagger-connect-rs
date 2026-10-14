#!/usr/bin/env bash
# Sourced by setup.sh. Guided port forwarding; the Rust configuration is shared.
guide_ask() {
  ask "$@"
  [[ ${REPLY,,} != q ]] || return 10
}
guide_port() {
  local label=$1 fallback=${2:-}
  while true; do
    guide_ask "$label — فقط عدد، مثال 2030 / port only" "$fallback"
    REPLY=$(python3 -c 'import sys; print(sys.argv[1].translate(str.maketrans("۰۱۲۳۴۵۶۷۸۹٠١٢٣٤٥٦٧٨٩", "01234567890123456789")))' "$REPLY")
    if [[ $REPLY =~ ^[0-9]{1,5}$ ]] && (( 10#$REPLY >= 1 && 10#$REPLY <= 65535 )); then
      port=$((10#$REPLY)); return
    fi
    echo 'پورت باید عدد 1 تا 65535 باشد؛ IP یا : وارد نکنید. / Invalid port.'
  done
}
guide_host() {
  local label=$1 fallback=${2:-} purpose=${3:-target}
  while true; do
    guide_ask "$label — بدون پورت / host only" "$fallback"
    if host=$(python3 - "$REPLY" "$purpose" <<'PY'
import ipaddress,re,sys
host,purpose=sys.argv[1:]
host=host.strip()
try:
    ip=ipaddress.ip_address(host.strip('[]'))
except ValueError:
    if (not re.fullmatch(r'(?=.{1,253}$)[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?',host)
        or host.isdigit() or re.fullmatch(r'[0-9.]+',host)
        or any(not label or len(label)>63 or label.startswith('-') or label.endswith('-') for label in host.split('.'))):
        raise SystemExit(1)
    if purpose=='iran' and (host.lower()=='localhost' or host.lower().endswith('.localhost')):
        raise SystemExit(1)
    print(host.lower())
else:
    if ip.is_unspecified or ip.is_multicast or (purpose=='iran' and (ip.is_loopback or ip.version!=4)):
        raise SystemExit(1)
    print(str(ip))
PY
    ); then return; fi
    echo 'آدرس معتبر وارد کنید، بدون http:// و بدون پورت. / Invalid host.'
    if [[ $purpose == iran ]]; then echo 'IPv4 قابل دسترس ایران؛ نه IP خارج، نه 127.0.0.1 و نه 0.0.0.0. برای تونل IPv6 از منوی پیشرفته استفاده کنید.'; fi
  done
}
guide_endpoint() {
  address=$1:$2
  [[ $1 != *:* ]] || address="[$1]:$2"
}
guide_role() {
  while true; do
    echo 'این فرمان را روی کدام سرور اجرا می‌کنید؟ / THIS machine'
    echo '1) ایران / Iran     2) خارج / Abroad     q) بازگشت'
    guide_ask 'شمارهٔ همین سرور / This machine number'
    case $REPLY in 1|server) role=server; return ;; 2|client) role=client; return ;; esac
    echo 'فقط 1 برای ایران یا 2 برای خارج وارد کنید.'
  done
}
guide_keys() {
  local here=ایران there=خارج other_option=2
  if [[ $role == client ]]; then here=خارج; there=ایران; other_option=1; fi
  printf '\nاین سرور: %s / %s\n' "$here" "$role"
  echo 'Client در این برنامه یعنی سرور خارج؛ منظور گوشی کاربر نیست.'
  ensure_keys
  printf 'کلید PUBLIC بالا متعلق به %s است؛ آن را در سرور %s وارد کنید.\n' "$here" "$there"
  printf 'اکنون کلید عمومی سرور %s را اینجا بچسبانید؛ نه کلید همین سرور.\n' "$there"
  printf 'برای گرفتن آن: در سرور %s گزینهٔ 15 و سپس نقش %s را بزنید.\n' "$there" "$other_option"
  echo 'اگر کلید طرف مقابل آماده نیست، Enter خالی = بازگشت؛ کلید همین سرور حفظ می‌شود.'
  while true; do
    guide_ask "کلید عمومی $there / OTHER machine public key (64 hex)"
    peer_key=${REPLY,,}
    [[ -n $peer_key ]] || return 10
    if [[ $peer_key =~ ^[0-9a-f]{64}$ && ! $peer_key =~ ^0+$ ]]; then
      local own_key
      own_key=$(tr -d '\r\n' < "$config_dir/$role-keys/public.key")
      [[ $peer_key == "${own_key,,}" ]] || return 0
      printf 'این کلید همین سرور %s است! کلید سرور %s را وارد کنید. / OTHER machine required.\n' "$here" "$there"
    else echo 'فقط رشتهٔ 64 کاراکتری Public key را کپی کنید؛ بدون عنوان و بدون Private key.'; fi
  done
}
guide_transport() {
  reverse=false
  echo
  echo 'روش تونل بین ایران و خارج؛ روی هر دو سرور باید یکسان باشد.'
  echo '1) TCP    2) Quantum+    3) KCP'
  echo 'برای شروع Enter = TCP. روش‌های دیگر در منوی پیشرفته، گزینهٔ 12 هستند.'
  while true; do
    guide_ask 'روش تونل / Same transport on BOTH servers' 1
    case ${REPLY,,} in
      1|tcp) transport=tcp; break ;;
      2|quantum+) transport=quantum+; break ;;
      3|kcp) transport=kcp; break ;;
      *) echo 'شمارهٔ 1، 2 یا 3 را وارد کنید.' ;;
    esac
  done
  extra=",\"transport\":\"$transport\""
}
guide_carrier() {
  local iran_host=0.0.0.0
  if [[ $role == client ]]; then
    echo 'از خارج به ایران وصل می‌شویم. IPv4 عمومی ایران یا دامنه‌ای که به IPv4 ایران اشاره دارد لازم است.'
    echo 'مثال قالب IPv4: 203.0.113.10 — آدرس واقعی ایران خودتان را وارد کنید.'
    guide_host 'IP یا دامنهٔ سرور ایران / IRAN server' '' iran
    iran_host=$host
  else
    echo 'ایران اتصال سرور خارج را دریافت می‌کند. آدرس شنود ایران خودکار 0.0.0.0 است.'
    echo '0.0.0.0 یعنی همهٔ رابط‌های همین سرور ایران؛ IP سرور خارج را اینجا وارد نمی‌کنید.'
  fi
  echo 'این پورت فقط برای ارتباط دو سرور است؛ با پورت کاربران یا اینباند اشتباه نشود.'
  echo 'روی هر دو سرور عدد یکسان وارد کنید. Enter = 7000.'
  guide_port 'پورت تونل روی ایران / IRAN tunnel port' 7000
  guide_endpoint "$iran_host" "$port"
}
guide_target() {
  echo
  echo 'مقصد نهایی = سرویس یا اینباندی که از قبل روی سرور خارج فعال است.'
  echo 'پورت صفحهٔ مدیریت پنل را وارد نکنید. این تونل خودش اینباند VPN نمی‌سازد.'
  echo '127.0.0.1 در این سؤال همیشه لوکالِ خارج است، حتی وقتی روی ایران تنظیم می‌کنید.'
  echo 'اگر سرویس روی خود سرور خارج است Enter بزنید؛ برای Docker/LAN آدرس قابل دسترس از خارج را وارد کنید.'
  guide_host 'آدرس سرویس از دید سرور خارج / Target host on ABROAD' 127.0.0.1 target
  local target_host=$host
  echo 'مثال: اگر اینباند خارج روی 2030 است، فقط 2030 بنویسید. مقدار پیش‌فرض ندارد.'
  guide_port 'پورت اینباند یا سرویس خارج / ABROAD service port'
  guide_endpoint "$target_host" "$port"
  target=$address
}
guide_mapping() {
  if [[ $role == server ]]; then
    echo
    echo 'نوع ترافیک سرویسِ خارج: معمولاً TCP؛ اگر اینباند شما UDP است، گزینهٔ 2.'
    while true; do
      guide_ask '1) TCP   2) UDP / Service traffic' 1
      case ${REPLY,,} in 1|tcp) kind=tcp; break ;; 2|udp) kind=udp; break ;; *) echo '1 یا 2 وارد کنید.' ;; esac
    done
    echo 'پورت ورودی کاربران روی ایران؛ در کانفیگ گوشی، IP ایران و همین پورت را می‌گذارید.'
    echo 'این پورت روی همهٔ رابط‌های ایران باز می‌شود. Enter = 18080.'
    while true; do
      guide_port 'پورت کاربران روی ایران / IRAN user port' 18080
      if [[ $port != "${carrier##*:}" || ( $transport == tcp && $kind == udp ) || ( $transport != tcp && $kind == tcp ) ]]; then break; fi
      echo 'پورت کاربران و پورت تونل با پروتکل یکسان روی ایران باید متفاوت باشند.'
    done
    bind=0.0.0.0:$port
  else
    echo 'در خارج فقط مقصد مجاز را مشخص می‌کنیم؛ عین آدرس و پورت مقصدی که روی ایران وارد کردید.'
  fi
  guide_target
}
guide_summary() {
  echo
  echo '========== بازبینی قبل از ذخیره / REVIEW =========='
  if [[ $role == server ]]; then
    echo 'این تنظیم روی: ایران / IRAN'
    printf 'ورودی کاربران روی ایران: %s (%s)\n' "$bind" "$kind"
    printf 'پورت اتصال سرور خارج به ایران: %s\n' "$carrier"
  else
    echo 'این تنظیم روی: خارج / ABROAD'
    printf 'خارج به این آدرس ایران وصل می‌شود: %s\n' "$carrier"
  fi
  printf 'روش تونل روی هر دو سرور: %s\n' "$transport"
  printf 'مقصد سرویس روی خارج (لوکال خارج، نه ایران): %s\n' "$target"
  echo 'آدرس و پورت مقصد خارج باید روی هر دو سرور دقیقاً یکسان باشد.'
  echo 'برای ذخیره 1؛ برای لغو بدون تغییر تنظیم قبلی 2؛ q = بازگشت.'
  guide_ask 'ذخیرهٔ این تنظیم؟ / Save this configuration? 1=yes 2=no' 1
  case ${REPLY,,} in 1|yes|y) ;; *) return 10 ;; esac
}
guide_finish() {
  echo
  if [[ $role == server ]]; then
    printf 'قدم بعد: روی خارج گزینهٔ 2، روش %s و پورت تونل %s را انتخاب کنید.\n' "$transport" "${carrier##*:}"
    printf 'در کانفیگ کاربر: IP عمومی ایران + پورت %s؛ سایر تنظیمات اینباند خارج حفظ شود.\n' "${bind##*:}"
  else
    echo 'قدم بعد: روی ایران گزینهٔ 1 را با کلید عمومی این سرور خارج تنظیم کنید.'
  fi
  printf 'مقصد یکسان در هر دو طرف: %s\n' "$target"
  echo 'برای بررسی اتصال و سرویس از گزینهٔ 13 استفاده کنید.'
}
