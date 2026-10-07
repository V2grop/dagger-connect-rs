# dagger-rs — راهنمای فارسی

بازنویسی مستقل تونل شبکه با Rust برای لینوکس. اعتبار نویسندهٔ اولیه: [ir_spoof](https://t.me/ir_spoof). مجوز MIT و اعلان‌های وابستگی‌ها حفظ شده‌اند.

## نسخه و سازگاری

نسخهٔ سورس موجود **0.2.1** است. **4.2.8-stable** نسخهٔ DaggerConnect مرجع است، نه نسخهٔ این بستهٔ Rust. این پروژه پروتکل احراز هویت و فریم‌بندی مستقل دارد و با کلاینت یا سرور اصلی DaggerConnect سازگار نیست.

در سورس مسیرهای TCP، KCP، HTTP/HTTPS، WS/WSS، XHTTP/XHTTPS، DC6، Quantum+، Quantum/Gaming و Raw TUN وجود دارند. وجود کد به معنی تأیید عملکرد در تمام شبکه‌ها نیست. محدودیت‌ها در [گزارش بازسازی](docs/reconstruction.md) و [مدل امنیتی](docs/security.md) آمده‌اند.

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
