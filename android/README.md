# WA Toplu Gönderici (Android)

Telefonda çalışan, sunucu gerektirmeyen toplu mesaj gönderici. Masaüstü
uygulamasından farkı: WhatsApp Web protokolünü kullanmaz, **telefonun kendi
WhatsApp uygulamasını erişilebilirlik (Accessibility) servisiyle otomatik
kullanır**.

## Neden protokol değil?

WhatsApp'ta bir hesap, birincil telefonuna en fazla 4 cihaz bağlar ve bu bağlama
**birincil telefonun kamerasıyla QR okutularak** yapılır. Aynı telefondaki bir
uygulama QR gösterip onu aynı telefonun kamerasıyla okutamayacağı için, "aynı
telefonda bağımsız gönderim" ancak telefonun kendi WhatsApp'ını otomatik
kullanmakla mümkündür. (Protokol yolu için ikinci bir telefon gerekir.)

## Nasıl çalışır

1. Rehberden kişileri seç (çoklu seçim, arama).
2. Mesajı yaz. `/isim` yazarsan alıcının adıyla değiştirilir (masaüstüyle aynı).
3. Bekleme aralığını ayarla (varsayılan 4-9 sn rastgele; her 25 mesajda uzun mola).
4. **Gönder**: uygulama her kişi için WhatsApp sohbetini açar, mesajı yazar,
   gönder düğmesine basar, geri döner, rastgele bekler ve sıradakine geçer.
5. İlerleme ve kişi başına sonuç ekranda; **Durdur** ile anında kesilir.

## Kurulum

1. Releases → `android-v0.1.0` → **app-debug.apk** dosyasını indir.
2. Telefonda "bilinmeyen kaynaklara izin ver" diyerek kur (APK debug anahtarıyla
   imzalıdır; Google Play'de yayınlanamaz).
3. Uygulamada sırayla izin ver:
   - **Rehber** (kişi seçimi)
   - **Bildirimler** (gönderim sürerken bildirim)
   - **Erişilebilirlik**: Ayarlar → Erişilebilirlik → WA Toplu Gönderici → Aç
   - **Diğer uygulamaların üzerinde göster** (arka plandan WhatsApp'ı açabilmek için)

## Uyarılar

- WhatsApp'ın "otomatik gönderim" davranışı hesap kısıtlamasına (ban) yol açabilir;
  çok sayıda mesajı kısa sürede göndermek riski artırır. Bekleme aralığını düşürmeyin.
- WhatsApp arayüzü (gönder düğmesinin yeri/adı) değişirse otomatik gönderim
  kırılabilir; bu durumda uygulamayı güncellemek gerekir.
- Yalnızca kendi telefonunda, kendi kişilerine mesaj göndermek için kullan.

## Derleme

CI: `.github/workflows/build-android.yml` (ubuntu + JDK 17 + Gradle 8.9,
`gradle assembleDebug`). Yerelde derlemek için Android SDK gerekir:

```bash
cd android && gradle assembleDebug
# çıktı: app/build/outputs/apk/debug/app-debug.apk
```
