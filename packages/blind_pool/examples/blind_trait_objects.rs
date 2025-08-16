//! Trait object usage with `BlindPool` using the macro-based approach.
//!
//! This example demonstrates the preferred way to work with trait objects:
//! using the `define_pooled_dyn_cast!` macro for type-safe trait object conversion.

use blind_pool::{BlindPool, define_pooled_dyn_cast};
use std::fmt::Display;

// Enable casting to Display trait objects
define_pooled_dyn_cast!(Display);

/// A trait for content that can be played as media.
pub trait MediaContent {
    /// Play the media content and return a playback message.
    fn play(&self) -> String;

    /// Get the duration of the content in seconds.
    fn duration_seconds(&self) -> u32;

    /// Get the title of the content.
    fn title(&self) -> &str;
}

// Enable casting to MediaContent trait objects
define_pooled_dyn_cast!(MediaContent);

// A media type that implements both Display and MediaContent.
#[derive(Debug)]
struct Song {
    title: String,
    artist: String,
    duration_seconds: u32,
}

impl Display for Song {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "'{}' by {}", self.title, self.artist)
    }
}

impl MediaContent for Song {
    fn play(&self) -> String {
        format!("🎵 Now playing: {self}")
    }

    fn duration_seconds(&self) -> u32 {
        self.duration_seconds
    }

    fn title(&self) -> &str {
        &self.title
    }
}

// Function that works with Display trait objects.
fn show_info(item: &dyn Display) {
    println!("Info: {item}");
}

// Function that works with MediaContent trait objects.
#[expect(
    clippy::integer_division,
    reason = "Time formatting requires integer division"
)]
fn play_media(content: &dyn MediaContent) {
    println!("{}", content.play());
    let duration = content.duration_seconds();
    println!("Duration: {}:{:02}", duration / 60, duration % 60);
    println!("Title: {}", content.title());
}

fn main() {
    println!("BlindPool Trait Object Example (Macro-based)");
    println!("============================================");

    // Create a pool.
    let pool = BlindPool::new();

    // Insert different types that implement Display.
    let song = Song {
        title: "Bohemian Rhapsody".to_string(),
        artist: "Queen".to_string(),
        duration_seconds: 355,
    };
    let number = 42_u32;
    let text = "Hello, World!".to_string();

    let song_handle = pool.insert(song);
    let number_handle = pool.insert(number);
    let text_handle = pool.insert(text);

    println!("Inserted items into pool");

    println!("\n=== Using Display trait objects ===");
    // Cast to Display trait objects using the macro (requires exclusive ownership).
    let song_display = song_handle.cast_display();
    show_info(&*song_display);

    let number_display = number_handle.cast_display();
    show_info(&*number_display);

    let text_display = text_handle.cast_display();
    show_info(&*text_display);

    println!("\n=== Using MediaContent trait objects ===");
    // Need to re-insert the song to cast to MediaContent since we consumed it above.
    let another_song = Song {
        title: "We Will Rock You".to_string(),
        artist: "Queen".to_string(),
        duration_seconds: 122,
    };
    let song_handle2 = pool.insert(another_song);
    let song_media = song_handle2.cast_media_content();
    play_media(&*song_media);

    println!("\n=== Storing trait objects in collections ===");
    let display_items: Vec<&dyn Display> = vec![
        &*number_display,
        &*text_display,
    ];

    for (i, item) in display_items.iter().enumerate() {
        println!("{i}: {item}");
    }

    println!("\nPool length: {}", pool.len());
    println!("All trait objects preserve reference counting!");
}
