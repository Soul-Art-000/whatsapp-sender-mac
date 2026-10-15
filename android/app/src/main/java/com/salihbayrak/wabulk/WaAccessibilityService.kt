package com.salihbayrak.wabulk

import android.accessibilityservice.AccessibilityService
import android.content.Intent
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo

/**
 * WhatsApp sohbet ekranındaki "gönder" düğmesine basmak için kullanılır.
 * Olay (event) tabanlı otomatik tıklama YAPMAZ; yalnızca SendService
 * clickSend() çağırdığında tıklar.
 */
class WaAccessibilityService : AccessibilityService() {

    companion object {
        @Volatile
        var instance: WaAccessibilityService? = null
    }

    override fun onServiceConnected() {
        super.onServiceConnected()
        instance = this
    }

    override fun onUnbind(intent: Intent?): Boolean {
        instance = null
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        instance = null
        super.onDestroy()
    }

    // Bilinçli olarak boş: tıklama sadece istek üzerine yapılır.
    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
    }

    override fun onInterrupt() {
    }

    /**
     * Verilen süre boyunca (100 ms arayla) ekran ağacını gezer,
     * gönder düğmesini bulup tıklar. Bulamazsa false döner.
     */
    fun clickSend(timeoutMs: Long): Boolean {
        val bitis = System.currentTimeMillis() + timeoutMs
        while (System.currentTimeMillis() < bitis) {
            val root = rootInActiveWindow
            if (root != null) {
                val dugum = findSend(root, 0)
                if (dugum != null) {
                    val ok = try {
                        dugum.performAction(AccessibilityNodeInfo.ACTION_CLICK)
                    } catch (e: Exception) {
                        false
                    }
                    if (ok) return true
                }
            }
            try {
                Thread.sleep(100)
            } catch (e: InterruptedException) {
                return false
            }
        }
        return false
    }

    /** Geri tuşu (sohbetten çıkıp listeye dönmek için). */
    fun goBack() {
        try {
            performGlobalAction(GLOBAL_ACTION_BACK)
        } catch (e: Exception) {
            // Servis kapanmış olabilir; sorun değil.
        }
    }

    // Ağacı özyinelemeli gezer; derinlik sınırı ile korunur.
    private fun findSend(node: AccessibilityNodeInfo?, depth: Int): AccessibilityNodeInfo? {
        if (node == null || depth > 40) return null
        if (isSend(node)) return node
        val adet = node.childCount
        for (i in 0 until adet) {
            val child = try {
                node.getChild(i)
            } catch (e: Exception) {
                null
            }
            val bulunan = findSend(child, depth + 1)
            if (bulunan != null) return bulunan
        }
        return null
    }

    private fun isSend(node: AccessibilityNodeInfo): Boolean {
        val id = node.viewIdResourceName
        if (id != null && id.endsWith("/send")) return true
        val desc = node.contentDescription?.toString()?.trim()?.lowercase()
        return desc == "send" || desc == "gönder"
    }
}
