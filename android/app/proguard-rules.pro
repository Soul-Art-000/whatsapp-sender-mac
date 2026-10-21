# Erişilebilirlik servisi manifest üzerinden yüklenir, adı korunmalı.
-keep class com.salihbayrak.wabulk.** { *; }
-keepnames class * extends android.accessibilityservice.AccessibilityService
-keepnames class * extends android.app.Service
-keepattributes *Annotation*
