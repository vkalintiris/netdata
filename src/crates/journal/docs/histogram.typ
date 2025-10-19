#set document(title: "My Rust Library Documentation")
#set page(numbering: "1")
#set align(left)
#set par(justify: true)

= Journal files indexing

We have written a library in Rust that is able to read and write systemd's
journal file format. On top of this library, we've developed types that allow
someone to index log directories that contain multiple journal files. This
will allow users to query the logs contained in journal files.

For the time being, we are primarily interested in allowing users to specify
a time range and create histograms for log entry fields.

Our code provides the `FileIndex` type that represents an index of a single
journal file:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileIndex {
    // The journal file's histogram
    pub histogram: Histogram,

    // Set of fields in the file
    pub fields: HashSet<String>,

    // Bitmap for each indexed field
    pub bitmaps: HashMap<String, Bitmap>,
}
```

The `FileIndex` contains all the fields that appear in a journal file. Also,
it keeps a sparse histogram which simply contains the duration of each bucket
and a vector of buckets:

```rust
/// An index structure for efficiently generating time-based histograms from journal entries.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Histogram {
    /// The duration of each bucket
    bucket_duration: u64,
    /// Sparse vector containing only bucket boundaries where changes occur.
    buckets: Vec<Bucket>,
}
```

Each bucket contains it's start time and the _running_ count of entries:

```rust
/// A time-aligned bucket in the file histogram.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Bucket {
    /// Start time of this bucket
    pub start_time: u64,
    /// Count of items in this bucket
    pub count: usize,
}
```

The choice to keep the _running_ count in buckets allows us to use a compact
representation of the entries in which a specific `field=value` appears in
the file. We use the `roaring` crate and provide the `Bitmap` _newtype_ on
top of the `RoaringBitmap` type.

Given the `Histogram` of a specific journal file, we can quickly generate the
histogram of a specific `field=value` by getting the `Bitmap` from the
`bitmaps` field. We do this by iterating each bucket of the `Histogram` and
iterating each set bit of the `Bitmap` in a nested loop. Whenever the set bit
in the `Bitmap` is larger than the _running_ count of the histogram bucket,
we create a new _bucket_ for the histogram of the `field=value`.

The chosen representation allows us to generate not only histograms of a
single `key=value`, but also a combination of them. For example, to generate
the histogram of `key1=value1 AND key2=value2`, we can simply perform the
intersection of the `key1=value1` bitmap with the bitmap of `key2=value2`. We
can then use the new `bitmap` to generate a histogram of log entries where
both `key1=value1` and `key2=value2` appears over time.

It should be noted that:

- A `Histogram`'s buckets are aligned to its bucket duration.
- Given a `Histogram` with a smaller bucket duration, we can generate a new
  one with a bucket duration that is a multiple of the original one.

The last point means that we can generate, for example, minute-aligned
histograms from a histogram that contains per-second aligned buckets.

== Installation

Add this to your `Cargo.toml`:
```toml
[dependencies]
my_crate = "0.1.0"
```

== Example Usage
```rust
use my_crate::MyStruct;

fn main() {
    let instance = MyStruct::new();
    instance.do_something();
}
```

= API Reference

#text(weight: "bold")[`MyStruct::new()`]

Creates a new instance...
