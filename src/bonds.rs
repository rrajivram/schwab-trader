//! Bonds held in the account: maturity value and the cash still to come
//! (remaining coupons plus principal), with payment dates moved off weekends
//! and US federal holidays the way Treasury pays them.

use chrono::{Datelike, Duration, Months, NaiveDate, Weekday};

use crate::accounts::{Account, Position};

/// Schwab counts bond quantity in $1,000-face units: 33 bonds at a price of
/// 99.58 showed a market value of $32,874 (verified live).
const FACE_PER_UNIT: f64 = 1000.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Bond {
    pub cusip: String,
    pub description: String,
    pub face: f64,
    /// Coupon in percent; 0 for bills.
    pub coupon_rate: f64,
    pub maturity: NaiveDate,
    pub cost: f64,
    pub market_value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentKind {
    Coupon,
    Maturity,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Payment {
    pub cusip: String,
    pub description: String,
    pub kind: PaymentKind,
    /// The scheduled date (coupon or maturity date).
    pub scheduled: NaiveDate,
    /// When the cash actually arrives: the next business day if `scheduled`
    /// is a weekend or federal holiday.
    pub paid: NaiveDate,
    pub amount: f64,
}

impl Bond {
    pub fn from_position(p: &Position) -> Option<Bond> {
        if p.asset_type != "FIXED_INCOME" || p.long_quantity <= 0.0 {
            return None;
        }
        let face = p.long_quantity * FACE_PER_UNIT;
        Some(Bond {
            cusip: p.cusip.clone().unwrap_or_else(|| p.symbol.clone()),
            description: p.description.clone().unwrap_or_else(|| p.symbol.clone()),
            face,
            coupon_rate: p.coupon_rate.unwrap_or(0.0),
            maturity: p.maturity_date?,
            cost: face * p.average_price / 100.0,
            market_value: p.market_value,
        })
    }

    pub fn is_bill(&self) -> bool {
        self.coupon_rate <= 0.0
    }

    /// One semiannual coupon payment.
    pub fn coupon_amount(&self) -> f64 {
        self.face * self.coupon_rate / 100.0 / 2.0
    }

    /// Coupon dates after `today` up to and including maturity, counting back
    /// six months at a time from the maturity date. A month-end maturity keeps
    /// its coupons on month ends (10/31 → 4/30).
    pub fn coupon_dates(&self, today: NaiveDate) -> Vec<NaiveDate> {
        if self.is_bill() {
            return Vec::new();
        }
        let month_end = is_month_end(self.maturity);
        let mut dates = Vec::new();
        for k in 0.. {
            let Some(d) = self.maturity.checked_sub_months(Months::new(6 * k)) else { break };
            let d = if month_end { last_day_of_month(d) } else { d };
            if d <= today {
                break;
            }
            dates.push(d);
        }
        dates.reverse();
        dates
    }

    /// Every payment still to come, oldest first.
    pub fn payments(&self, today: NaiveDate) -> Vec<Payment> {
        let make = |kind, scheduled: NaiveDate, amount| Payment {
            cusip: self.cusip.clone(),
            description: self.description.clone(),
            kind,
            scheduled,
            paid: next_business_day(scheduled),
            amount,
        };
        let mut out: Vec<Payment> =
            self.coupon_dates(today).into_iter().map(|d| make(PaymentKind::Coupon, d, self.coupon_amount())).collect();
        if self.maturity > today {
            out.push(make(PaymentKind::Maturity, self.maturity, self.face));
        }
        out
    }

    pub fn remaining_coupons(&self, today: NaiveDate) -> f64 {
        self.coupon_dates(today).len() as f64 * self.coupon_amount()
    }
}

pub fn bonds_in(account: &Account) -> Vec<Bond> {
    account.positions.iter().filter_map(Bond::from_position).collect()
}

/// All upcoming payments across bonds, by the date the cash arrives.
pub fn upcoming(bonds: &[Bond], today: NaiveDate) -> Vec<Payment> {
    let mut all: Vec<Payment> = bonds.iter().flat_map(|b| b.payments(today)).collect();
    all.sort_by(|a, b| a.paid.cmp(&b.paid).then(b.amount.total_cmp(&a.amount)));
    all
}

fn is_month_end(d: NaiveDate) -> bool {
    (d + Duration::days(1)).month() != d.month()
}

fn last_day_of_month(d: NaiveDate) -> NaiveDate {
    let first_next = if d.month() == 12 {
        NaiveDate::from_ymd_opt(d.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(d.year(), d.month() + 1, 1)
    };
    first_next.expect("valid date") - Duration::days(1)
}

pub fn next_business_day(d: NaiveDate) -> NaiveDate {
    let mut d = d;
    while matches!(d.weekday(), Weekday::Sat | Weekday::Sun) || is_federal_holiday(d) {
        d += Duration::days(1);
    }
    d
}

/// Nth `weekday` of a month (n = 1..5), or the last one when n is 0.
fn nth_weekday(year: i32, month: u32, weekday: Weekday, n: u32) -> NaiveDate {
    if n == 0 {
        let mut d = last_day_of_month(NaiveDate::from_ymd_opt(year, month, 1).expect("valid"));
        while d.weekday() != weekday {
            d -= Duration::days(1);
        }
        return d;
    }
    NaiveDate::from_weekday_of_month_opt(year, month, weekday, n as u8).expect("valid")
}

/// US federal holidays (the days Treasury doesn't pay), with fixed-date
/// holidays observed on the Friday before / Monday after a weekend.
pub fn is_federal_holiday(d: NaiveDate) -> bool {
    let y = d.year();
    let observed = |m: u32, day: u32| {
        let h = NaiveDate::from_ymd_opt(y, m, day).expect("valid");
        match h.weekday() {
            Weekday::Sat => h - Duration::days(1),
            Weekday::Sun => h + Duration::days(1),
            _ => h,
        }
    };
    let fixed = [observed(1, 1), observed(6, 19), observed(7, 4), observed(11, 11), observed(12, 25)];
    let floating = [
        nth_weekday(y, 1, Weekday::Mon, 3),  // Martin Luther King Jr. Day
        nth_weekday(y, 2, Weekday::Mon, 3),  // Washington's Birthday
        nth_weekday(y, 5, Weekday::Mon, 0),  // Memorial Day
        nth_weekday(y, 9, Weekday::Mon, 1),  // Labor Day
        nth_weekday(y, 10, Weekday::Mon, 2), // Columbus Day
        nth_weekday(y, 11, Weekday::Thu, 4), // Thanksgiving
    ];
    // Next year's New Year's Day observed on Dec 31 when Jan 1 is a Saturday.
    let next_new_year = NaiveDate::from_ymd_opt(y + 1, 1, 1).expect("valid");
    let ny_on_dec31 = next_new_year.weekday() == Weekday::Sat && d == next_new_year - Duration::days(1);
    fixed.contains(&d) || floating.contains(&d) || ny_on_dec31
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn bond(rate: f64, maturity: NaiveDate) -> Bond {
        Bond {
            cusip: "X".into(),
            description: "test".into(),
            face: 33_000.0,
            coupon_rate: rate,
            maturity,
            cost: 0.0,
            market_value: 0.0,
        }
    }

    #[test]
    fn weekend_maturities_pay_next_business_day() {
        // Your 4.125% note matures Sat 10/31/2026; the 2% note Sun 11/15/2026.
        assert_eq!(next_business_day(ymd(2026, 10, 31)), ymd(2026, 11, 2));
        assert_eq!(next_business_day(ymd(2026, 11, 15)), ymd(2026, 11, 16));
        assert_eq!(next_business_day(ymd(2026, 11, 27)), ymd(2026, 11, 27)); // day after Thanksgiving is a business day
    }

    #[test]
    fn federal_holidays_shift_payments() {
        assert!(is_federal_holiday(ymd(2026, 11, 26))); // Thanksgiving
        assert!(is_federal_holiday(ymd(2026, 11, 11))); // Veterans Day
        assert!(is_federal_holiday(ymd(2027, 1, 18))); // MLK Day
        assert!(is_federal_holiday(ymd(2026, 7, 3))); // July 4 on a Saturday, observed Friday
        assert_eq!(next_business_day(ymd(2027, 1, 18)), ymd(2027, 1, 19));
    }

    #[test]
    fn note_maturing_soon_has_only_its_final_coupon() {
        let b = bond(4.125, ymd(2026, 10, 31));
        let p = b.payments(ymd(2026, 10, 1));
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].kind, PaymentKind::Coupon);
        assert!((p[0].amount - 680.625).abs() < 1e-9);
        assert_eq!(p[0].paid, ymd(2026, 11, 2));
        assert_eq!(p[1].kind, PaymentKind::Maturity);
        assert_eq!(p[1].amount, 33_000.0);
    }

    #[test]
    fn month_end_coupons_stay_on_month_ends() {
        let b = bond(4.0, ymd(2028, 10, 31));
        let d = b.coupon_dates(ymd(2026, 10, 1));
        assert_eq!(d.first(), Some(&ymd(2026, 10, 31)));
        assert!(d.contains(&ymd(2027, 4, 30)));
        assert_eq!(d.len(), 5); // 10/26, 4/27, 10/27, 4/28, 10/28
    }

    #[test]
    fn bills_pay_only_face_at_maturity() {
        let b = bond(0.0, ymd(2026, 11, 5));
        let p = b.payments(ymd(2026, 10, 1));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].amount, 33_000.0);
        assert_eq!(b.remaining_coupons(ymd(2026, 10, 1)), 0.0);
    }

    #[test]
    fn matured_bond_has_nothing_left() {
        assert!(bond(2.0, ymd(2026, 9, 30)).payments(ymd(2026, 10, 1)).is_empty());
    }
}
