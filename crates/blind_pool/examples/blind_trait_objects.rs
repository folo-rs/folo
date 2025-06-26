//! Trait object usage with `BlindPool`.
//!
//! This example demonstrates how to use trait objects with `BlindPool`, showing:
//! * Storing completely different types in the same pool
//! * Creating references from raw pointers
//! * Converting those references to trait object references
//! * Using trait methods on pooled items

use std::fmt::Display;

use blind_pool::BlindPool;

// Define a trait for our multimedia content.
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

// Different media types.
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

impl Display for Song {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "'{} - {}' ({}:{:02})",
            self.title,
            self.artist,
            self.duration_seconds / 60,
            self.duration_seconds % 60
        )
    }
}

#[derive(Debug)]
struct Video {
    title: String,
    resolution: String,
    duration_seconds: u32,
    rating: f32,
}

impl MediaContent for Video {
    fn play(&self) -> String {
        format!("🎬 Now playing: '{}' in {}", self.title, self.resolution)
    }

    fn duration_seconds(&self) -> u32 {
        self.duration_seconds
    }

    fn title(&self) -> &str {
        &self.title
    }
}

impl Ratable for Video {
    fn get_rating(&self) -> f32 {
        self.rating
    }

    fn set_rating(&mut self, rating: f32) {
        self.rating = rating.clamp(0.0, 5.0);
    }
}

impl Display for Video {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "'{}' ({}, {}:{:02})",
            self.title,
            self.resolution,
            self.duration_seconds / 60,
            self.duration_seconds % 60
        )
    }
}

#[derive(Debug)]
struct Podcast {
    title: String,
    host: String,
    episode_number: u32,
    duration_seconds: u32,
    rating: f32,
}

impl MediaContent for Podcast {
    fn play(&self) -> String {
        format!(
            "🎙️ Now playing: '{}' Episode {} with {}",
            self.title, self.episode_number, self.host
        )
    }

    fn duration_seconds(&self) -> u32 {
        self.duration_seconds
    }

    fn title(&self) -> &str {
        &self.title
    }
}

impl Ratable for Podcast {
    fn get_rating(&self) -> f32 {
        self.rating
    }

    fn set_rating(&mut self, rating: f32) {
        self.rating = rating.clamp(0.0, 5.0);
    }
}

impl Display for Podcast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "'{}' Ep.{} with {} ({}:{:02})",
            self.title,
            self.episode_number,
            self.host,
            self.duration_seconds / 60,
            self.duration_seconds % 60
        )
    }
}

fn main() {
    println!("BlindPool Trait Object Example");
    println!("==============================");
    println!();

    // Create a single blind pool to store all different media types.
    let mut media_pool = BlindPool::new();

    println!("Creating a multimedia library with different content types...");
    println!();

    // Insert different media types into the same pool.
    let song = Song {
        title: "Bohemian Rhapsody".to_string(),
        artist: "Queen".to_string(),
        duration_seconds: 354,
        rating: 4.8,
    };

    let video = Video {
        title: "The Matrix".to_string(),
        resolution: "1080p".to_string(),
        duration_seconds: 8160, // 136 minutes
        rating: 4.5,
    };

    let podcast = Podcast {
        title: "Tech Talk".to_string(),
        host: "Alice Smith".to_string(),
        episode_number: 42,
        duration_seconds: 3600, // 60 minutes
        rating: 4.2,
    };

    // Store handles and their types.
    // In a real application, you might use an enum or a more sophisticated system.
    enum MediaHandle {
        Song(blind_pool::Pooled<Song>),
        Video(blind_pool::Pooled<Video>),
        Podcast(blind_pool::Pooled<Podcast>),
    }

    let song_handle = MediaHandle::Song(media_pool.insert(song));
    let video_handle = MediaHandle::Video(media_pool.insert(video));
    let podcast_handle = MediaHandle::Podcast(media_pool.insert(podcast));

    let handles = vec![song_handle, video_handle, podcast_handle];

    println!("Added {} items to the media pool", media_pool.len());
    println!("Pool capacity: {}", media_pool.capacity());
    println!();

    // Example 1: Use media content via trait objects.
    println!("Example 1: Playing media content");
    println!("--------------------------------");

    fn play_media(content: &dyn MediaContent) {
        println!("{}", content.play());
        let duration = content.duration_seconds();
        println!(
            "Duration: {}:{:02}",
            duration / 60,
            duration % 60
        );
        println!();
    }

    for handle in &handles {
        match handle {
            MediaHandle::Song(pooled) => unsafe {
                // SAFETY: The pointer is valid and points to a Song that we inserted.
                let song_ref: &Song = pooled.ptr().as_ref();
                let media: &dyn MediaContent = song_ref;
                play_media(media);
            },
            MediaHandle::Video(pooled) => unsafe {
                // SAFETY: The pointer is valid and points to a Video that we inserted.
                let video_ref: &Video = pooled.ptr().as_ref();
                let media: &dyn MediaContent = video_ref;
                play_media(media);
            },
            MediaHandle::Podcast(pooled) => unsafe {
                // SAFETY: The pointer is valid and points to a Podcast that we inserted.
                let podcast_ref: &Podcast = pooled.ptr().as_ref();
                let media: &dyn MediaContent = podcast_ref;
                play_media(media);
            },
        }
    }

    // Example 2: Modify ratings via mutable trait objects.
    println!("Example 2: Updating ratings");
    println!("---------------------------");

    fn rate_content(content: &mut dyn Ratable, new_rating: f32) {
        let old_rating = content.get_rating();
        content.set_rating(new_rating);
        println!(
            "Rating updated: {:.1} → {:.1}",
            old_rating,
            content.get_rating()
        );
    }

    for handle in &handles {
        match handle {
            MediaHandle::Song(pooled) => unsafe {
                // SAFETY: The pointer is valid and points to a Song that we inserted.
                let song_ref: &mut Song = pooled.ptr().as_mut();
                let ratable: &mut dyn Ratable = song_ref;
                print!("Song rating: ");
                rate_content(ratable, 5.0);
            },
            MediaHandle::Video(pooled) => unsafe {
                // SAFETY: The pointer is valid and points to a Video that we inserted.
                let video_ref: &mut Video = pooled.ptr().as_mut();
                let ratable: &mut dyn Ratable = video_ref;
                print!("Video rating: ");
                rate_content(ratable, 4.7);
            },
            MediaHandle::Podcast(pooled) => unsafe {
                // SAFETY: The pointer is valid and points to a Podcast that we inserted.
                let podcast_ref: &mut Podcast = pooled.ptr().as_mut();
                let ratable: &mut dyn Ratable = podcast_ref;
                print!("Podcast rating: ");
                rate_content(ratable, 4.0);
            },
        }
    }

    println!();

    // Example 3: Display all content using separate trait objects.
    println!("Example 3: Content summary");
    println!("-------------------------");

    fn summarize_content(media: &dyn MediaContent, displayable: &dyn Display) {
        println!("📱 {}", displayable);
        println!("   {}", media.play());
        println!("   Duration: {}:{:02}", 
                 media.duration_seconds() / 60,
                 media.duration_seconds() % 60);
    }

    for handle in &handles {
        match handle {
            MediaHandle::Song(pooled) => unsafe {
                // SAFETY: The pointer is valid and points to a Song that we inserted.
                let song_ref: &Song = pooled.ptr().as_ref();
                let media: &dyn MediaContent = song_ref;
                let display: &dyn Display = song_ref;
                summarize_content(media, display);
            },
            MediaHandle::Video(pooled) => unsafe {
                // SAFETY: The pointer is valid and points to a Video that we inserted.
                let video_ref: &Video = pooled.ptr().as_ref();
                let media: &dyn MediaContent = video_ref;
                let display: &dyn Display = video_ref;
                summarize_content(media, display);
            },
            MediaHandle::Podcast(pooled) => unsafe {
                // SAFETY: The pointer is valid and points to a Podcast that we inserted.
                let podcast_ref: &Podcast = pooled.ptr().as_ref();
                let media: &dyn MediaContent = podcast_ref;
                let display: &dyn Display = podcast_ref;
                summarize_content(media, display);
            },
        }
        println!();
    }

    // Example 4: Type-erased handles.
    println!("Example 4: Type-erased handles");
    println!("------------------------------");

    // Collect type-erased handles and remove items in separate steps.
    let mut erased_handles = Vec::new();

    for handle in handles {
        match handle {
            MediaHandle::Song(pooled) => {
                let erased = pooled.erase();
                erased_handles.push(erased);
            },
            MediaHandle::Video(pooled) => {
                let erased = pooled.erase();
                erased_handles.push(erased);
            },
            MediaHandle::Podcast(pooled) => {
                let erased = pooled.erase();
                erased_handles.push(erased);
            },
        }
    }

    println!("Successfully converted all handles to type-erased form");
    println!("Pool still contains {} items", media_pool.len());

    // Clean up using the erased handles.
    for erased in erased_handles {
        media_pool.remove(erased);
    }

    println!("Pool is now empty: {}", media_pool.is_empty());

    println!();
    println!("Example completed successfully!");
    println!();
    println!("Key insights:");
    println!("- BlindPool can store completely different types in the same pool");
    println!("- Trait objects work by creating references from raw pointers");
    println!("- The caller must track the concrete type of each pooled item");
    println!("- Multiple traits can be used via separate trait object references");
    println!("- Type erasure is possible while maintaining functionality");
}