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

Çekirdeği GUI'siz denemek için: `cargo run -p taskman-core --example top`

## Neler var

- **İşlemler**
  - Windows'taki gibi üç grup: **Uygulamalar / Arka plan işlemleri / Sistem
    işlemleri** (systemd cgroup `app.slice` + uid sezgiseliyle)
  - 1 sn aralıkla canlı liste; sütun başlığına tıklayarak sıralama
  - Yazarak arama (ad veya PID)
  - "Görevi sonlandır" (SIGTERM); seçim, liste yeniden sıralansa da **PID ile korunur**
- **Performans**
  - Son 60 saniyenin CPU grafiği + çekirdek başına anlık yük çubukları
  - Bellek kullanım çubuğu
- CPU yüzdesi makine kapasitesine göredir (tüm çekirdekler = %100), Windows ile aynı.
- Kernel thread'leri gizlenir; tablo modeli her saniye yeniden yaratılmak yerine
  yerinde güncellenir.

## Bilinen PoC sınırları

- Çok süreçli uygulamalar tek satırda toplanmıyor (Windows'taki genişleyebilir
  gruplar F2+).
- Gruplama sezgiseli systemd tabanlı: terminalden başlatılan bazı süreçler
  "Arka plan"a düşebilir.
- Bellek RSS'tir (paylaşılan sayfalar dahil); PSS daha sonra.
- Örnekleme UI thread'inde; nihai sürümde ayrı thread'e taşınacak.
