# taskman (geçici ad) — PoC

Windows 11 Görev Yöneticisi'nin bilgi mimarisini ve davranışlarını örnek alan,
açık kaynak, dağıtımdan bağımsız bir Linux görev yöneticisi için **çalışan kavram
kanıtı (proof of concept)**.

Teknoloji: **Rust + Slint** (fluent stili). Mimari, ilk günden nihai projedeki
gibi ikiye ayrık:

- `core/` — arayüzden bağımsız veri toplama katmanı (`/proc` okuma, sinyal
  gönderme). Lisans hedefi: MIT/Apache-2.0 (başka projeler de kullanabilsin).
- `app/` — Slint arayüzü. Lisans hedefi: GPL-3.0-or-later.

## Çalıştırma

```sh
cargo run --release
```

## PoC'de neler var

- **İşlemler**: 1 sn aralıkla canlı süreç listesi (ad, PID, CPU %, bellek),
  CPU'ya göre sıralı; yazarak arama; "Görevi sonlandır" (SIGTERM).
- **Performans**: son 60 saniyenin CPU grafiği, bellek kullanım çubuğu.
- Windows'taki gibi CPU yüzdesi makine kapasitesine göredir (tüm çekirdekler = %100).
- Kernel thread'leri gizlenir (Windows'un süreç görünümü gibi).

## Bilinen PoC sınırları

- Uygulama gruplama (Uygulamalar / Arka plan) yok — F1 kapsamı.
- Sıralama sabit (CPU), sütun tıklamayla sıralama yok.
- Liste her saniye yenilendiğinde seçim satır numarasında kalır (kayabilir).
- Bellek RSS'tir (paylaşılan sayfalar dahil); PSS daha sonra.
- Örnekleme UI thread'inde (hızlı olduğu için); nihai sürümde ayrı thread'e taşınacak.
