use crate::config::BatteryColorMap;
pub(super) fn validate_thresholds(value: &str) -> Result<Vec<u8>, &'static str> {
    let values = value
        .split(',')
        .map(str::trim)
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("Enter whole percentages separated by commas.");
            }
            let percentage = part
                .parse::<u8>()
                .map_err(|_| "Each percentage must be from 1 to 100.")?;
            if !(1..=100).contains(&percentage) {
                return Err("Each percentage must be from 1 to 100.");
            }
            Ok(percentage)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if values.is_empty() {
        Err("Enter at least one battery percentage.")
    } else {
        Ok(values)
    }
}

pub(super) fn validate_cooldown(value: &str) -> Result<u64, &'static str> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Enter a whole number of minutes.");
    }
    let minutes = value
        .parse::<u64>()
        .map_err(|_| "The repeat interval is too large.")?;
    if minutes > i64::MAX as u64 {
        return Err("The repeat interval is too large.");
    }
    Ok(minutes)
}

pub(super) fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u8::from_str_radix(&digits[0..2], 16).ok()?,
        u8::from_str_radix(&digits[2..4], 16).ok()?,
        u8::from_str_radix(&digits[4..6], 16).ok()?,
    ])
}

pub(super) fn validate_color_map(
    red_cutoff: &str,
    yellow_cutoff: &str,
    red: &str,
    yellow: &str,
    green: &str,
) -> Result<BatteryColorMap, &'static str> {
    if !red_cutoff.bytes().all(|byte| byte.is_ascii_digit()) || red_cutoff.is_empty() {
        return Err("Enter a red cutoff from 0 to 99.");
    }
    if !yellow_cutoff.bytes().all(|byte| byte.is_ascii_digit()) || yellow_cutoff.is_empty() {
        return Err("Enter a yellow cutoff from 1 to 100.");
    }
    let red_max_percent = red_cutoff
        .parse::<u8>()
        .map_err(|_| "Enter a red cutoff from 0 to 99.")?;
    let yellow_max_percent = yellow_cutoff
        .parse::<u8>()
        .map_err(|_| "Enter a yellow cutoff from 1 to 100.")?;
    if red_max_percent > 99 {
        return Err("The red cutoff must be from 0 to 99.");
    }
    if !(1..=100).contains(&yellow_max_percent) {
        return Err("The yellow cutoff must be from 1 to 100.");
    }
    if red_max_percent >= yellow_max_percent {
        return Err("The red cutoff must be lower than the yellow cutoff.");
    }
    let red = parse_hex_color(red).ok_or("Use a hex color in #RRGGBB format.")?;
    let yellow = parse_hex_color(yellow).ok_or("Use a hex color in #RRGGBB format.")?;
    let green = parse_hex_color(green).ok_or("Use a hex color in #RRGGBB format.")?;
    Ok(BatteryColorMap {
        red_max_percent,
        yellow_max_percent,
        red,
        yellow,
        green,
    })
}

pub(super) fn hex_color([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

#[cfg(test)]
mod tests {
    use super::{parse_hex_color, validate_color_map, validate_cooldown, validate_thresholds};

    #[test]
    fn threshold_validation_accepts_multiple_whole_percentages() {
        assert_eq!(validate_thresholds("20, 10, 5"), Ok(vec![20, 10, 5]));
    }

    #[test]
    fn threshold_validation_rejects_empty_and_out_of_range_entries() {
        assert!(validate_thresholds("").is_err());
        assert!(validate_thresholds("20,,5").is_err());
        assert!(validate_thresholds("0, 5").is_err());
        assert!(validate_thresholds("101").is_err());
    }

    #[test]
    fn repeat_interval_accepts_zero_and_rejects_overflow() {
        assert_eq!(validate_cooldown("0"), Ok(0));
        assert!(validate_cooldown("-1").is_err());
        assert!(validate_cooldown("9223372036854775808").is_err());
    }

    #[test]
    fn color_map_accepts_hex_colors_and_ordered_cutoffs() {
        let map = validate_color_map("15", "30", "#D23030", "#ebbe23", "#2DAA4B").unwrap();
        assert_eq!(map.red_max_percent, 15);
        assert_eq!(map.yellow_max_percent, 30);
        assert_eq!(map.color_for(15), [210, 48, 48]);
        assert_eq!(map.color_for(30), [235, 190, 35]);
        assert_eq!(map.color_for(31), [232, 190, 36]);
        assert_eq!(map.color_for(100), [45, 170, 75]);
        assert_eq!(parse_hex_color("#123AbC"), Some([18, 58, 188]));
    }

    #[test]
    fn color_map_rejects_invalid_hex_and_overlapping_cutoffs() {
        assert!(validate_color_map("30", "30", "#D23030", "#EBBE23", "#2DAA4B").is_err());
        assert!(validate_color_map("100", "30", "#D23030", "#EBBE23", "#2DAA4B").is_err());
        assert!(validate_color_map("15", "30", "red", "#EBBE23", "#2DAA4B").is_err());
    }
}
