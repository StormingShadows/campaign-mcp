use anyhow::{anyhow, bail, Result};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiceRollResult {
    pub expression: String,
    pub count: u32,
    pub sides: u32,
    pub keep_mode: Option<String>,
    pub rolls: Vec<i64>,
    pub kept: Vec<i64>,
    pub modifier: i64,
    pub total: i64,
    pub is_natural_max: bool,
    pub is_natural_one: bool,
}

/// Parse and evaluate standard RPG dice notation:
/// Examples: "1d20", "2d6+3", "4d6kh3", "2d20kl1-2", "d100", "3d8 + 4".
pub fn roll_dice(expression: &str, seed: Option<u64>) -> Result<DiceRollResult> {
    let compact: String = expression
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect();

    if compact.is_empty() {
        bail!("Empty dice expression");
    }

    // Split optional +/- modifier at the end
    let (dice_part, modifier) = split_modifier(&compact)?;

    // Parse [count]d<sides>[kh<n>|kl<n>]
    let d_pos = dice_part
        .find('d')
        .ok_or_else(|| anyhow!("Invalid dice expression '{expression}': missing 'd'"))?;

    let count_str = &dice_part[..d_pos];
    let count: u32 = if count_str.is_empty() {
        1
    } else {
        count_str
            .parse()
            .map_err(|_| anyhow!("Invalid dice count '{count_str}' in '{expression}'"))?
    };

    if count == 0 || count > 1000 {
        bail!("Dice count must be between 1 and 1000 (got {count})");
    }

    let rest = &dice_part[d_pos + 1..];
    let (sides_str, keep_mode, keep_n) = if let Some(idx) = rest.find("kh") {
        let k: usize = rest[idx + 2..]
            .parse()
            .map_err(|_| anyhow!("Invalid keep-highest count in '{expression}'"))?;
        (&rest[..idx], Some(format!("kh{k}")), Some((true, k)))
    } else if let Some(idx) = rest.find("kl") {
        let k: usize = rest[idx + 2..]
            .parse()
            .map_err(|_| anyhow!("Invalid keep-lowest count in '{expression}'"))?;
        (&rest[..idx], Some(format!("kl{k}")), Some((false, k)))
    } else {
        (rest, None, None)
    };

    let sides: u32 = sides_str
        .parse()
        .map_err(|_| anyhow!("Invalid dice sides '{sides_str}' in '{expression}'"))?;

    if sides < 2 || sides > 10_000 {
        bail!("Dice sides must be between 2 and 10000 (got {sides})");
    }

    let mut rng: StdRng = match seed {
        Some(s) => StdRng::seed_from_u64(s),
        None => StdRng::from_entropy(),
    };

    let mut rolls = Vec::with_capacity(count as usize);
    for _ in 0..count {
        rolls.push(rng.gen_range(1..=(sides as i64)));
    }

    let kept = if let Some((highest, k)) = keep_n {
        if k == 0 || k > rolls.len() {
            bail!(
                "Keep count {k} must be between 1 and dice count {}",
                rolls.len()
            );
        }
        let mut sorted = rolls.clone();
        sorted.sort_unstable();
        if highest {
            sorted.reverse();
        }
        sorted.into_iter().take(k).collect::<Vec<_>>()
    } else {
        rolls.clone()
    };

    let sum: i64 = kept.iter().sum();
    let total = sum + modifier;

    let is_natural_max = kept.len() == 1 && kept[0] == sides as i64;
    let is_natural_one = kept.len() == 1 && kept[0] == 1;

    Ok(DiceRollResult {
        expression: expression.trim().to_string(),
        count,
        sides,
        keep_mode,
        rolls,
        kept,
        modifier,
        total,
        is_natural_max,
        is_natural_one,
    })
}

fn split_modifier(s: &str) -> Result<(&str, i64)> {
    // Find last '+' or '-' after index 0
    if let Some(pos) = s.rfind(|c| c == '+' || c == '-') {
        if pos > 0 {
            let head = &s[..pos];
            let mod_str = &s[pos..];
            let modifier: i64 = mod_str
                .parse()
                .map_err(|_| anyhow!("Invalid dice modifier '{mod_str}' in '{s}'"))?;
            return Ok((head, modifier));
        }
    }
    Ok((s, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_rolls() {
        let r1 = roll_dice("2d20kh1+5", Some(42)).unwrap();
        let r2 = roll_dice("2d20kh1+5", Some(42)).unwrap();
        assert_eq!(r1.rolls, r2.rolls);
        assert_eq!(r1.kept.len(), 1);
        assert_eq!(r1.kept[0], *r1.rolls.iter().max().unwrap());
        assert_eq!(r1.total, r1.kept[0] + 5);
    }

    #[test]
    fn test_keep_lowest_and_negative_mod() {
        let r = roll_dice("4d6kl2 - 3", Some(7)).unwrap();
        assert_eq!(r.rolls.len(), 4);
        assert_eq!(r.kept.len(), 2);
        assert_eq!(r.modifier, -3);
        assert_eq!(r.total, r.kept.iter().sum::<i64>() - 3);
    }
}
