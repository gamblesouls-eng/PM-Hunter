package com.pokehunt

import com.pokehunt.core.Hunter
import com.pokehunt.core.Phone
import com.pokehunt.core.Rect
import com.pokehunt.core.Shot
import com.pokehunt.core.Text
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.file.Files

/** The Rust hunter (libhunt, built for this computer) through the Kotlin the
 *  app uses: a pretend phone showing samples/quaxly-shiny.png, the enemy's box
 *  "read" as Quaxly. It should tap Run, and the battle's over. */
class BridgeTest {
    private val samples = File("../../samples")

    private fun shot(name: String): Shot {
        val img = Class.forName("javax.imageio.ImageIO").getMethod("read", File::class.java).invoke(null, File(samples, name))
        val c = img.javaClass
        val w = c.getMethod("getWidth").invoke(img) as Int
        val h = c.getMethod("getHeight").invoke(img) as Int
        val px = c.getMethod("getRGB", Int::class.java, Int::class.java, Int::class.java, Int::class.java,
                             IntArray::class.java, Int::class.java, Int::class.java)
            .invoke(img, 0, 0, w, h, null, 0, w) as IntArray
        val rgba = ByteArray(w * h * 4)
        for (i in px.indices) {
            rgba[i * 4] = (px[i] shr 16).toByte()
            rgba[i * 4 + 1] = (px[i] shr 8).toByte()
            rgba[i * 4 + 2] = px[i].toByte()
            rgba[i * 4 + 3] = -1
        }
        return Shot(w, h, rgba)
    }

    inner class Pretend : Phone {
        val battle = shot("quaxly-shiny.png")
        val calm = Shot(battle.width, battle.height, ByteArray(battle.rgba.size) { 60 })  // (not black: the screen's up)
        @Volatile var inBattle = false
        val taps = ArrayList<Pair<Double, Double>>()
        val said = ArrayList<String>()
        var code: String? = null

        override fun grab(): Shot {
            Thread.sleep(5)
            return if (inBattle) battle else calm
        }
        override fun tap(x: Double, y: Double) {
            synchronized(taps) { taps.add(x to y) }
            if (Math.abs(x - 1568) < 5 && Math.abs(y - 1070) < 5) inBattle = false  // Run
        }
        override fun swipe(x0: Double, y0: Double, x1: Double, y1: Double, seconds: Double) {}
        override fun typeText(text: String) = true
        override fun keyboardHidden(hidden: Boolean) {}
        override fun robloxInFront() = true
        override fun focusRoblox() {}
        override fun buttonBox(): Rect? = null
        override fun placeButton(mute: Rect) {}
        override fun idle(): Double? = null
        // the enemy's box is 520 px wide on this screen
        override fun ocr(shot: Shot): List<Text> =
            if (shot.width == 520) listOf(Text("Quaxly", 0.2, 0.15), Text("Lv. 7", 0.7, 0.15)) else emptyList()
        override fun sound(kind: String) {}
        override fun savePicture(png: ByteArray, name: String) = "-"
        override fun label() = "test phone"
        override fun websiteLogin() {}
        override fun say(line: String) { synchronized(said) { said.add(line) } }
        override fun code(code: String?) { this.code = code }
    }

    @Test fun runsFromAnUnwantedOne() {
        val phone = Pretend()
        val dir = Files.createTempDirectory("hunt").toFile()
        File(dir, "debug-run.json").writeText("{}")  // not a new install: no debug run first
        val hunter = Hunter(phone, """{"targets": [{"species": "Lapras"}]}""", dir.path, false, false, "", "test")
        val thread = Thread { hunter.run() }.apply { start() }
        Thread.sleep(3000)  // (a phone waits 2 s past its first picture: a battle on screen then is yours)
        phone.inBattle = true
        val end = System.currentTimeMillis() + 20000
        while (System.currentTimeMillis() < end && phone.inBattle) Thread.sleep(50)
        hunter.stop()
        thread.join(10000)
        val said = synchronized(phone.said) { phone.said.joinToString("\n") }
        assertTrue(said, said.contains("shiny Quaxly Lv7"))
        assertTrue(said, synchronized(phone.taps) { phone.taps.any { Math.abs(it.first - 1568) < 5 } })
        assertTrue(said, said.contains("stopped."))
        assertTrue(File(dir, "encounters.csv").readText().contains("Quaxly"))
    }

    @Test fun makesAKey() {
        assertEquals(43, com.pokehunt.core.newKey().length)
    }
}
