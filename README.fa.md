# dagger-rs — راهنمای فارسی

بازنویسی مستقل تونل شبکه با Rust برای لینوکس. اعتبار نویسندهٔ اولیه: [ir_spoof](https://t.me/ir_spoof). مجوز MIT و اعلان‌های وابستگی‌ها حفظ شده‌اند.

## نسخه و سازگاری

نسخهٔ سورس موجود **0.2.1** است. **4.2.8-stable** نسخهٔ DaggerConnect مرجع است، نه نسخهٔ این بستهٔ Rust. این پروژه پروتکل احراز هویت و فریم‌بندی مستقل دارد و با کلاینت یا سرور اصلی DaggerConnect سازگار نیست.

در سورس مسیرهای TCP، KCP، HTTP/HTTPS، WS/WSS، XHTTP/XHTTPS، DC6، Quantum+، Quantum/Gaming و Raw TUN وجود دارند. وجود کد به معنی تأیید عملکرد در تمام شبکه‌ها نیست. محدودیت‌ها در [گزارش بازسازی](docs/reconstruction.md) و [مدل امنیتی](docs/security.md) آمده‌اند.

## نصب از GitHub Releases

پس از انتشار موفق بسته در [Releases](https://github.com/V2grop/dagger-connect-rs/releases)، روی هر سرور Ubuntu/Debian با معماری x86-64 اجرا کنید:

```sh
sudo apt-get update && sudo apt-get install -y ca-certificates curl python3
curl -fL https://raw.githubusercontent.com/V2grop/dagger-connect-rs/main/scripts/install-release.sh -o install-dagger.sh && sudo bash install-dagger.sh
```

اسکریپت نسخهٔ منتشرشده را انتخاب می‌کند، فایل نصب و checksum همان نسخه را می‌گیرد، صحت فایل را بررسی و منوی تنظیمات را باز می‌کند. اگر هنوز Release ساخته نشده باشد، نصب متوقف می‌شود. دفعات بعد:

```sh
sudo /usr/local/bin/dagger-setup
```

برای تونل ساده، ایران `server` و خارج `client` است. روی هر طرف گزینهٔ ۱ را برای کلیدهای همان نقش اجرا کنید؛ فقط کلید عمومی را مبادله کنید. در گزینهٔ ۲، هر دو طرف کلید عمومی طرف مقابل را وارد کنند. ایران: `tcp`، `ports`، شنود `0.0.0.0:7000`، فوروارد TCP با bind `0.0.0.0:18080` و target `127.0.0.1:8080`. خارج: `tcp`، `ports`، اتصال به `IRAN_IP:7000`، مقصد مجاز `127.0.0.1:8080` و pool برابر ۱. سرویس مقصد باید روی خارج فعال باشد. در ایران پورت ۷۰۰۰/TCP را برای IP خارج و ۱۸۰۸۰/TCP را برای کاربران مجاز باز کنید. روی هر طرف گزینهٔ ۳ برای اعتبارسنجی و سپس گزینهٔ ۵ برای سرویس systemd؛ وضعیت با ۶ و لاگ با ۹.

## ساخت

روی لینوکس با Rust پایدار و ابزارهای ساخت بومی:

```sh
cargo build --locked --release
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --release --all-targets
python3 -m unittest discover -s tests -p 'test_install.py' -v
```

باینری حاصل در `target/release/dagger-rs` است. راهنمای انگلیسی روش بسته‌بندی و تست‌های شبکه با دسترسی ویژه را توضیح می‌دهد.

## راه‌اندازی دو میزبان

روی سرور و کلاینت کلید جداگانه بسازید:

```sh
# سرور
target/release/dagger-rs keygen --out keys/server
# کلاینت
target/release/dagger-rs keygen --out keys/client
```

نمونه‌های `examples/server.json` و `examples/client.json` را در پوشهٔ `config` کپی کنید. مسیر کلید خصوصی هر میزبان، کلید عمومی طرف مقابل، آدرس سرور و پورت مقصد را تنظیم کنید. کلید خصوصی را منتقل نکنید. مسیر فایل‌ها نسبت به فایل تنظیمات تفسیر می‌شود.

```sh
target/release/dagger-rs --config config/server.json --check
target/release/dagger-rs --config config/server.json
# روی کلاینت، از config/client.json استفاده کنید.
```

`allowed_targets` خالی همهٔ مقصدهای فوروارد را رد می‌کند؛ `*` همه را مجاز می‌کند. SOCKS5 فقط CONNECT دارد؛ احراز هویت کاربری و UDP ASSOCIATE پیاده نشده‌اند. TUN به CAP_NET_ADMIN و مسیرهای خام به CAP_NET_RAW نیاز دارند. پورت‌های فوروارد و SOCKS را با فایروال محدود کنید.

## اصلاحات نگه‌داری

نصب‌کننده هنگام جایگزینی `dagger-setup` دیگر فایل مقصد لینک نمادین را بازنویسی نمی‌کند. سه تست نصب و راه‌اندازی مجدد به CI اضافه شده است. کامپایل Rust و آزمون مستقل پروتکل‌ها در محیط این بررسی اجرا نشده‌اند؛ جزئیات در [گزارش بررسی](docs/maintenance-review.md) هستند.


## منوی دوم با سبک نسخهٔ اصلی

منوی جدید را V2grop برای هستهٔ مستقل Rust طراحی کرده است. چیدمان آن از منوی
itsFLoKi/DaggerConnect الهام گرفته؛ هسته، مجوز یا سازگاری نسخهٔ اصلی را اضافه نمی‌کند.
منوی قبلی همچنان موجود است. هر دو منو از کلیدها، تنظیمات و سرویس‌های یکسان استفاده می‌کنند.

روی Ubuntu/Debian با معماری x86-64:

```bash
curl -fLO https://raw.githubusercontent.com/V2grop/dagger-connect-rs/main/setup.sh
chmod +x setup.sh
sudo ./setup.sh
```

این ورودی آخرین ریلیز V2grop را با بررسی SHA256 نصب می‌کند و منوی دوم را باز می‌کند.
نصب مجدد تنظیمات را حفظ می‌کند؛ سرویس‌های در حال اجرا خودکار ری‌استارت نمی‌شوند.

```bash
sudo /usr/local/bin/dagger-setup-classic  # منوی دوم
sudo /usr/local/bin/dagger-setup          # منوی قبلی
```

گزینه‌های ۱ و ۲ نقش سرور را انتخاب کرده، کلید عمومی و تنظیمات تونل را می‌گیرند.
پس از آن نصب سرویس با انتخاب شما انجام می‌شود. گزینهٔ ۱۲ منوی قبلی را برای
تولید کلید، گواهی TLS و تنظیمات پیشرفته باز می‌کند. حذف فقط سرویس انتخابی را
حذف می‌کند و کلیدها و فایل‌های تنظیمات را نگه می‌دارد.
