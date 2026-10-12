//! 宽容反序列化助手：服务端在进度/内容接口中会返回显式 `null`、浮点（如 0.75）
//! 或数字字符串，裸类型 + `#[serde(default)]` 无法处理这些情况。

use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// 字段缺失或显式 `null` 时使用类型默认值（适用于字符串/布尔等）。
pub fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn value_to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f.round() as i64)),
        Value::String(s) => {
            let s = s.trim();
            s.parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().map(|f| f.round() as i64))
        }
        _ => None,
    }
}

fn parse_value<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Value>::deserialize(deserializer)
}

/// 字符串字段：缺失/null → ""，数字/布尔自动转字符串。
pub fn tolerant_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match parse_value(deserializer)? {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s,
        Some(other) => other.to_string(),
    })
}

/// 数值字段：缺失/null/浮点四舍五入/数字字符串均可，非法值取 0。
pub fn tolerant_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(parse_value(deserializer)?
        .as_ref()
        .and_then(value_to_i64)
        .unwrap_or(0))
}

/// 同 `tolerant_i64`，结果收敛到 i32 范围。
pub fn tolerant_i32<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(parse_value(deserializer)?
        .as_ref()
        .and_then(value_to_i64)
        .unwrap_or(0)
        .clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

/// 同 `tolerant_i64`，结果收敛到 u8 范围。
pub fn tolerant_u8<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(parse_value(deserializer)?
        .as_ref()
        .and_then(value_to_i64)
        .unwrap_or(0)
        .clamp(0, u8::MAX as i64) as u8)
}

/// 可空数值字段（Option<i64>）：null/缺失 → None，浮点/字符串照常解析。
pub fn tolerant_opt_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(parse_value(deserializer)?.as_ref().and_then(value_to_i64))
}

/// 可空数值字段（Option<i32>）：null/缺失 → None，浮点四舍五入。
pub fn tolerant_opt_i32<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(parse_value(deserializer)?
        .as_ref()
        .and_then(value_to_i64)
        .map(|v| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Case {
        #[serde(default, deserialize_with = "tolerant_i64")]
        a: i64,
        #[serde(default, deserialize_with = "tolerant_i32")]
        b: i32,
        #[serde(default, deserialize_with = "tolerant_u8")]
        c: u8,
        #[serde(default, deserialize_with = "tolerant_opt_i64")]
        d: Option<i64>,
        #[serde(default, deserialize_with = "tolerant_opt_i32")]
        e: Option<i32>,
    }

    fn parse(v: serde_json::Value) -> Case {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn tolerant_numbers_accept_null_float_string() {
        let c = parse(serde_json::json!({"a": null, "b": 0.75, "c": 1.0, "d": "123", "e": 60.6}));
        assert_eq!(c.a, 0);
        assert_eq!(c.b, 1);
        assert_eq!(c.c, 1);
        assert_eq!(c.d, Some(123));
        assert_eq!(c.e, Some(61));

        let c = parse(serde_json::json!({}));
        assert_eq!((c.a, c.b, c.c, c.d, c.e), (0, 0, 0, None, None));

        let c = parse(serde_json::json!({"a": 120.5, "b": "60", "c": null, "d": null, "e": "0.75"}));
        assert_eq!(c.a, 121); // round
        assert_eq!(c.b, 60);
        assert_eq!(c.c, 0);
        assert_eq!(c.d, None);
        assert_eq!(c.e, Some(1));

        let c = parse(serde_json::json!({"a": -3.6, "b": "abc", "c": 300}));
        assert_eq!(c.a, -4);
        assert_eq!(c.b, 0);
        assert_eq!(c.c, 255); // clamp
    }

    #[test]
    fn null_default_helper() {
        #[derive(serde::Deserialize)]
        struct S {
            #[serde(default, deserialize_with = "null_default")]
            s: String,
            #[serde(default, deserialize_with = "null_default")]
            b: bool,
        }
        let v: S = serde_json::from_value(serde_json::json!({"s": null, "b": null})).unwrap();
        assert!(v.s.is_empty() && !v.b);
        let v: S = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(v.s.is_empty());
    }
}
