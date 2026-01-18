use little_exif::metadata::Metadata;
use little_exif::exif_tag::ExifTag;
use little_exif::rational::uR64;
use serde::Deserialize;

/// Represents a timestamp in Google's supplemental metadata format
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoogleTimestamp {
    /// Unix timestamp as a string
    pub timestamp: String,
    /// Human-readable formatted date
    pub formatted: String,
}

/// Represents geo data in Google's supplemental metadata format
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeoData {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: f64,
    #[serde(default)]
    pub latitude_span: f64,
    #[serde(default)]
    pub longitude_span: f64,
}

/// Represents the origin information for how a photo was added to Google Photos
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GooglePhotosOrigin {
    #[serde(default)]
    pub web_upload: Option<serde_json::Value>,
    #[serde(default)]
    pub mobile_upload: Option<serde_json::Value>,
    #[serde(default)]
    pub from_partner_sharing: Option<serde_json::Value>,
}

/// Represents the Google Photos supplemental metadata JSON format
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoogleSupplementalMetadata {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub image_views: Option<String>,
    pub creation_time: Option<GoogleTimestamp>,
    pub photo_taken_time: Option<GoogleTimestamp>,
    pub geo_data: Option<GeoData>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub google_photos_origin: Option<GooglePhotosOrigin>,
    #[serde(default)]
    pub geo_data_exif: Option<GeoData>,
    #[serde(default)]
    pub people: Option<serde_json::Value>,
    #[serde(default)]
    pub enrichments: Option<serde_json::Value>,
    #[serde(default)]
    pub favorited: Option<bool>,
    #[serde(default)]
    pub archived: Option<bool>,
    #[serde(default)]
    pub trashed: Option<bool>,
    #[serde(default)]
    pub app_source: Option<serde_json::Value>,
}

/// Error type for metadata operations
#[derive(Debug)]
pub enum MetadataError {
    /// JSON parsing failed, includes the raw JSON for debugging
    JsonParseError {
        message: String,
        json: String,
    },
    /// Unknown field encountered in metadata
    UnknownField {
        field: String,
        json: String,
    },
    InvalidTimestamp(String),
}

impl std::fmt::Display for MetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetadataError::JsonParseError { message, json } => {
                write!(
                    f,
                    "Failed to parse Google metadata JSON.\n\
                    \n\
                    Error: {}\n\
                    \n\
                    This may indicate an unknown metadata format from Google Takeout.\n\
                    Please create a GitHub issue at:\n\
                    https://github.com/YOUR_USERNAME/takeout-fixer/issues/new\n\
                    \n\
                    Include the following JSON in your report:\n\
                    ---\n\
                    {}\n\
                    ---",
                    message, json
                )
            }
            MetadataError::UnknownField { field, json } => {
                write!(
                    f,
                    "Unknown field '{}' found in Google metadata.\n\
                    \n\
                    This tool may need to be updated to handle new Google Takeout formats.\n\
                    Please create a GitHub issue at:\n\
                    https://github.com/YOUR_USERNAME/takeout-fixer/issues/new\n\
                    \n\
                    Include the following JSON in your report:\n\
                    ---\n\
                    {}\n\
                    ---",
                    field, json
                )
            }
            MetadataError::InvalidTimestamp(msg) => write!(f, "Invalid timestamp: {}", msg),
        }
    }
}

impl std::error::Error for MetadataError {}

/// Parses Google supplemental metadata JSON and updates an existing Metadata object.
///
/// # Arguments
/// * `json` - The JSON string containing Google supplemental metadata
/// * `metadata` - The existing Metadata object to update
///
/// # Returns
/// The updated Metadata object, or an error if parsing fails
pub fn apply_google_metadata(
    json: &str,
    mut metadata: Metadata,
) -> Result<Metadata, MetadataError> {
    let google_meta: GoogleSupplementalMetadata = serde_json::from_str(json).map_err(|e| {
        let error_msg = e.to_string();
        // Check if it's an unknown field error
        if error_msg.contains("unknown field") {
            // Try to extract the field name from the error message
            let field = error_msg
                .split("unknown field `")
                .nth(1)
                .and_then(|s| s.split('`').next())
                .unwrap_or("unknown")
                .to_string();
            MetadataError::UnknownField {
                field,
                json: json.to_string(),
            }
        } else {
            MetadataError::JsonParseError {
                message: error_msg,
                json: json.to_string(),
            }
        }
    })?;

    // Apply description if present and non-empty
    if !google_meta.description.is_empty() {
        metadata.set_tag(ExifTag::ImageDescription(google_meta.description));
    }

    // Apply photo taken time if present
    if let Some(ref photo_time) = google_meta.photo_taken_time {
        if let Ok(timestamp) = photo_time.timestamp.parse::<i64>() {
            let datetime = format_exif_datetime(timestamp);
            metadata.set_tag(ExifTag::DateTimeOriginal(datetime));
        }
    }

    // Apply GPS coordinates if present and valid (non-zero)
    if let Some(ref geo) = google_meta.geo_data {
        if geo.latitude != 0.0 || geo.longitude != 0.0 {
            // Convert latitude to EXIF format (degrees, minutes, seconds as rationals)
            let (lat_ref, lat_vals) = decimal_to_dms_exif(geo.latitude, true);
            let (lon_ref, lon_vals) = decimal_to_dms_exif(geo.longitude, false);

            metadata.set_tag(ExifTag::GPSLatitudeRef(lat_ref));
            metadata.set_tag(ExifTag::GPSLatitude(lat_vals));
            metadata.set_tag(ExifTag::GPSLongitudeRef(lon_ref));
            metadata.set_tag(ExifTag::GPSLongitude(lon_vals));

            // Apply altitude if non-zero
            if geo.altitude != 0.0 {
                let alt_ref = if geo.altitude >= 0.0 { 0u8 } else { 1u8 };
                let alt_val = uR64 {
                    nominator: (geo.altitude.abs() * 1000.0) as u32,
                    denominator: 1000,
                };
                metadata.set_tag(ExifTag::GPSAltitudeRef(vec![alt_ref]));
                metadata.set_tag(ExifTag::GPSAltitude(vec![alt_val]));
            }
        }
    }

    Ok(metadata)
}

/// Formats a Unix timestamp as an EXIF datetime string (YYYY:MM:DD HH:MM:SS)
fn format_exif_datetime(timestamp: i64) -> String {

    // Convert to a simple date/time representation
    // Note: This is a simplified implementation; for production use chrono crate
    let secs_since_epoch = timestamp;
    let days_since_epoch = secs_since_epoch / 86400;
    let time_of_day = secs_since_epoch % 86400;

    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Simplified date calculation (doesn't account for leap years perfectly)
    let (year, month, day) = days_to_ymd(days_since_epoch);

    format!(
        "{:04}:{:02}:{:02} {:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

/// Converts days since Unix epoch to year, month, day
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    // Start from 1970-01-01
    let mut remaining_days = days;
    let mut year = 1970i32;

    // Find the year
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    // Find the month and day
    let days_in_months: [i64; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u32;
    for days_in_month in days_in_months.iter() {
        if remaining_days < *days_in_month {
            break;
        }
        remaining_days -= days_in_month;
        month += 1;
    }

    let day = remaining_days as u32 + 1;

    (year, month, day)
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Converts decimal degrees to EXIF DMS format (degrees, minutes, seconds as rationals)
/// Returns (reference string "N"/"S" or "E"/"W", vector of uR64 rationals)
fn decimal_to_dms_exif(decimal: f64, is_latitude: bool) -> (String, Vec<uR64>) {
    let reference = if is_latitude {
        if decimal >= 0.0 { "N" } else { "S" }
    } else {
        if decimal >= 0.0 { "E" } else { "W" }
    };

    let abs_decimal = decimal.abs();
    let degrees = abs_decimal.floor() as u32;
    let minutes_float = (abs_decimal - degrees as f64) * 60.0;
    let minutes = minutes_float.floor() as u32;
    let seconds_float = (minutes_float - minutes as f64) * 60.0;

    // Store seconds with high precision (multiply by 1000 for 3 decimal places)
    let seconds_num = (seconds_float * 1000.0).round() as u32;

    let vals = vec![
        uR64 { nominator: degrees, denominator: 1 },
        uR64 { nominator: minutes, denominator: 1 },
        uR64 { nominator: seconds_num, denominator: 1000 },
    ];

    (reference.to_string(), vals)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"{
        "title": "IMG_8238.JPG",
        "description": "A beautiful sunset",
        "imageViews": "0",
        "creationTime": {
            "timestamp": "1587036746",
            "formatted": "16. apr. 2020, 11.32.26 UTC"
        },
        "photoTakenTime": {
            "timestamp": "1563032119",
            "formatted": "13. jul. 2019, 15.35.19 UTC"
        },
        "geoData": {
            "latitude": 46.7234,
            "longitude": 17.3456,
            "altitude": 150.5,
            "latitudeSpan": 0.0,
            "longitudeSpan": 0.0
        },
        "url": "https://photos.google.com/photo/test"
    }"#;

    #[test]
    fn test_parse_google_metadata() {
        let meta: GoogleSupplementalMetadata = serde_json::from_str(SAMPLE_JSON).unwrap();
        assert_eq!(meta.title, "IMG_8238.JPG");
        assert_eq!(meta.description, "A beautiful sunset");
        assert!(meta.photo_taken_time.is_some());
        assert!(meta.geo_data.is_some());

        let geo = meta.geo_data.unwrap();
        assert!((geo.latitude - 46.7234).abs() < 0.0001);
        assert!((geo.longitude - 17.3456).abs() < 0.0001);
    }

    #[test]
    fn test_format_exif_datetime() {
        // 1563032119 = 2019-07-13 15:35:19 UTC
        let result = format_exif_datetime(1563032119);
        assert_eq!(result, "2019:07:13 15:35:19");
    }

    #[test]
    fn test_decimal_to_dms() {
        let (lat_ref, lat_vals) = decimal_to_dms_exif(46.7234, true);
        assert_eq!(lat_ref, "N");
        assert_eq!(lat_vals.len(), 3);
        assert_eq!(lat_vals[0], uR64 { nominator: 46, denominator: 1 }); // 46 degrees

        let (lon_ref, _) = decimal_to_dms_exif(-17.3456, false);
        assert_eq!(lon_ref, "W");
    }

    #[test]
    fn test_apply_google_metadata() {
        let metadata = Metadata::new();
        let result = apply_google_metadata(SAMPLE_JSON, metadata);
        assert!(result.is_ok());
    }
}
