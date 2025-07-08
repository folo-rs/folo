//! Trait object usage with `BlindPool`.
//!
//! This example demonstrates how to use trait objects with `BlindPool`, showing:
//! * Storing a concrete type in the pool
//! * Creating references from raw pointers
//! * Converting those references to trait object references
//! * Using trait methods on pooled items

use blind_pool::BlindPool;

// Define a trait for our content.
trait MediaContent {
    fn play(&self) -> String;
    fn duration_seconds(&self) -> u32;
    fn title(&self) -> &str;
}

// Define another trait for content that can be rated.
trait Ratable {
    fn get_rating(&self) -> f32;
    fn set_rating(&mut self, rating: f32);
}

// A media type.
#[derive(Debug)]
struct Song {
    title: String,
    artist: String,
    duration_seconds: u32,
    rating: f32,
}

impl MediaContent for Song {
    fn play(&self) -> String {
        format!("🎵 Now playing: '{}' by {}", self.title, self.artist)
    }

    fn duration_seconds(&self) -> u32 {
        self.duration_seconds
    }

    fn title(&self) -> &str {
        &self.title
    }
}

impl Ratable for Song {
    fn get_rating(&self) -> f32 {
        self.rating
    }

    fn set_rating(&mut self, rating: f32) {
        self.rating = rating.clamp(0.0, 5.0);
    }
}

// Function that works with any MediaContent trait object.
#[expect(
    clippy::integer_division,
    reason = "Time formatting requires integer division"
)]
fn play_media(content: &dyn MediaContent) {
    println!("{}", content.play());
    let duration = content.duration_seconds();
    println!("Duration: {}:{:02}", duration / 60, duration % 60);
    println!();
}

// Function that modifies rating via mutable trait objects.
fn rate_content(content: &mut dyn Ratable, new_rating: f32) {
    let old_rating = content.get_rating();
    content.set_rating(new_rating);
    println!(
        "Rating updated: {:.1} → {:.1}",
        old_rating,
        content.get_rating()
    );
}

fn main() {
    println!("BlindPool Trait Object Example");
    println!("==============================");
    println!();

    // Create a blind pool to store media content.
    let mut media_pool = BlindPool::new();

    println!("Creating a multimedia library...");
    println!();

    // Insert a song into the pool.
    let song = Song {
        title: "Bohemian Rhapsody".to_string(),
        artist: "Queen".to_string(),
        duration_seconds: 354,
        rating: 4.8,
    };

    let song_handle = media_pool.insert(song);

    println!("Added song to the media pool");
    println!("Song capacity: {}", media_pool.capacity_of::<Song>());
    println!();

    // Example 1: Use media content via trait objects.
    println!("Example 1: Playing media content");
    println!("--------------------------------");

    // SAFETY: The pointer is valid and points to a Song that we inserted.
    unsafe {
        let song_ref: &Song = song_handle.ptr().as_ref();
        let media: &dyn MediaContent = song_ref;
        play_media(media);
    }

    // Example 2: Modify rating via mutable trait objects.
    println!("Example 2: Updating rating");
    println!("-------------------------");

    // SAFETY: The pointer is valid and points to a Song that we inserted.
    unsafe {
        let song_ref: &mut Song = song_handle.ptr().as_mut();
        let ratable: &mut dyn Ratable = song_ref;
        print!("Song rating: ");
        rate_content(ratable, 5.0);
    }

    println!();

    // Example 3: Using multiple trait objects on the same item.
    println!("Example 3: Multiple trait objects");
    println!("---------------------------------");

    // SAFETY: The pointer is valid and points to a Song that we inserted.
    unsafe {
        let song_ref: &Song = song_handle.ptr().as_ref();
        let media: &dyn MediaContent = song_ref;
        println!("Title: {}", media.title());
    }

    // SAFETY: The pointer is valid and points to a Song that we inserted.
    let rating = unsafe {
        let song_ref: &Song = song_handle.ptr().as_ref();
        let ratable: &dyn Ratable = song_ref;
        ratable.get_rating()
    };
    println!("Rating: {rating:.1}/5.0");

    println!();

    // Clean up.
    media_pool.remove(song_handle);

    println!("Pool is now empty: {}", media_pool.is_empty());

    println!();
    println!("Example completed successfully!");
    println!();
    println!("Key insights:");
    println!("- BlindPool can store any type and convert to trait objects");
    println!("- Trait objects work by creating references from raw pointers");
    println!("- Multiple traits can be used via separate trait object references");
    println!("- The caller must track the concrete type of each pooled item");
}
