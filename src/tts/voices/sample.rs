//! Per-language pangram sample texts. Pangrams exercise a wide phoneme
//! range so the user can actually hear the difference between two
//! similar voices in a 2-3 second audition.

use super::catalog::VoiceEntry;

/// Map a BCP-47 code (or just the language-prefix) to a pangram-style
/// sample. Unknown languages fall back to the English pangram.
pub fn pangram_for(lang: &str) -> &'static str {
    let key = lang.split('-').next().unwrap_or("en").to_ascii_lowercase();
    match key.as_str() {
        "en" => {
            "The quick brown fox jumps over the lazy dog. She sells sea shells by the seashore."
        }
        "ja" => "これは日本語の音声サンプルです。今日の天気は晴れで、風が少し冷たいです。",
        "zh" | "cmn" | "yue" => "天地玄黄，宇宙洪荒。日月盈昃，辰宿列张。",
        "ko" => "다람쥐 헌 쳇바퀴에 타고파.",
        "de" => "Falsches Üben von Xylophonmusik quält jeden größeren Zwerg.",
        "fr" => "Portez ce vieux whisky au juge blond qui fume.",
        "es" => "El veloz murciélago hindú comía feliz cardillo y kiwi.",
        "it" => "Ma la volpe, col suo balzo, ha raggiunto il quieto Fido.",
        "pt" => "Luís argüia à Júlia que «brações, fé, chá, óxido, pôr, zângão» eram do português.",
        "ru" => "Съешь же ещё этих мягких французских булок, да выпей чаю.",
        "nl" => "Pa's wijze lynx bezag vroom het fikse aquaduct.",
        "pl" => "Pchnąć w tę łódź jeża lub ośm skrzyń fig.",
        "sv" => "Flygande bäckasiner söka hwila på mjuka tuvor.",
        "ar" => "نص حكيم له سر قاطع وذو شأن عظيم مكتوب على ثوب أخضر ومغلف بجلد أزرق.",
        "hi" => "ऋषियों को सताने वाले दुष्ट राक्षसों के राजा रावण का सर्वनाश करने वाले विष्णुवतार भगवान श्रीराम।",
        "tr" => "Pijamalı hasta yağız şoföre çabucak güvendi.",
        "cs" => "Příliš žluťoučký kůň úpěl ďábelské ódy.",
        "el" => "Ξεσκεπάζω την ψυχοφθόρα βδελυγμία.",
        "fi" => "Albert osti fagotin ja töräytti puhkuvan melodian.",
        "vi" => "Con cáo nâu nhanh nhẹn nhảy qua con chó lười biếng.",
        "th" => "นายสังฆภัณฑ์ เฮงพิทักษ์ฝั่ง ผู้เฒ่าซึ่งมีอาชีพเป็นฅนขายฃวด ถูกตำรวจปฏิบัติการจับฟ้องศาล",
        _ => "The quick brown fox jumps over the lazy dog.",
    }
}

/// Pick the language key used to look up the sample text for a given
/// voice. Multilingual voices always get English; otherwise the voice's
/// first listed language wins.
pub fn sample_lang_for(entry: &VoiceEntry) -> &str {
    if entry.multilingual {
        return "en";
    }
    entry.primary_language()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_languages_have_native_samples() {
        assert!(pangram_for("ja-JP").contains("音声サンプル"));
        assert!(pangram_for("de-DE").contains("Xylophon"));
        assert!(pangram_for("fr-FR").contains("whisky"));
    }

    #[test]
    fn unknown_language_falls_back_to_english() {
        let sample = pangram_for("xx-YY");
        assert!(sample.contains("fox"));
    }

    #[test]
    fn chinese_variants_share_sample() {
        let sample_zh = pangram_for("zh-CN");
        let sample_cmn = pangram_for("cmn-CN");
        let sample_yue = pangram_for("yue-HK");
        assert_eq!(sample_zh, sample_cmn);
        assert_eq!(sample_zh, sample_yue);
    }
}
