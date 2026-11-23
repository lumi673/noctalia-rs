use std::f64::consts::PI;

const SYNODIC_MONTH: f64 = 29.530588853;
const NEW_MOON_EPOCH: f64 = 2415021.076998695;

fn int(x: f64) -> i64 {
    x.floor() as i64
}

pub fn jd_from_date(dd: i64, mm: i64, yy: i64) -> f64 {
    let a = int((14 - mm) as f64 / 12.0);
    let y = yy + 4800 - a;
    let m = mm + 12 * a - 3;

    let mut jd = dd as f64
        + int((153 * m + 2) as f64 / 5.0) as f64
        + 365.0 * y as f64
        + int(y as f64 / 4.0) as f64
        - int(y as f64 / 100.0) as f64
        + int(y as f64 / 400.0) as f64
        - 32045.0;

    if jd < 2299161.0 {
        jd = dd as f64
            + int((153 * m + 2) as f64 / 5.0) as f64
            + 365.0 * y as f64
            + int(y as f64 / 4.0) as f64
            - 32083.0;
    }

    jd
}

fn new_moon(k: f64) -> f64 {
    let t = k / 1236.85;
    let t2 = t * t;
    let t3 = t2 * t;
    let dr = PI / 180.0;

    let mut jd1 =
        2415020.75933 + 29.53058868 * k + 0.0001178 * t2 - 0.000000155 * t3;

    jd1 += 0.00033 * ((166.56 + 132.87 * t - 0.009173 * t2) * dr).sin();

    let mv = 359.2242 + 29.10535608 * k - 0.0000333 * t2 - 0.00000347 * t3;
    let mpr = 306.0253 + 385.81691806 * k + 0.0107306 * t2 + 0.00001236 * t3;
    let f = 21.2964 + 390.67050646 * k - 0.0016528 * t2 - 0.00000239 * t3;

    let c1 = (0.1734 - 0.000393 * t) * (mv * dr).sin()
        + 0.0021 * (2.0 * mv * dr).sin()
        - 0.4068 * (mpr * dr).sin()
        + 0.0161 * (2.0 * mpr * dr).sin()
        - 0.0004 * (3.0 * mpr * dr).sin()
        + 0.0104 * (2.0 * f * dr).sin()
        - 0.0051 * ((mv + mpr) * dr).sin()
        - 0.0074 * ((mv - mpr) * dr).sin()
        + 0.0004 * ((2.0 * f + mv) * dr).sin()
        - 0.0004 * ((2.0 * f - mv) * dr).sin()
        - 0.0006 * ((2.0 * f + mpr) * dr).sin()
        + 0.0010 * ((2.0 * f - mpr) * dr).sin()
        + 0.0005 * ((2.0 * mpr + mv) * dr).sin();

    let delta_t = if t < -11.0 {
        0.001 + 0.000839 * t + 0.0002261 * t2
            - 0.00000845 * t3
            - 0.000000081 * t * t3
    } else {
        -0.000278 + 0.000265 * t + 0.000262 * t2
    };

    jd1 + c1 - delta_t
}

fn sun_longitude(jdn: f64) -> f64 {
    let t = (jdn - 2451545.0) / 36525.0;
    let t2 = t * t;
    let dr = PI / 180.0;

    let mv = 357.52910 + 35999.05030 * t - 0.0001559 * t2 - 0.00000048 * t * t2;
    let l0 = 280.46645 + 36000.76983 * t + 0.0003032 * t2;

    let mut dl = (1.914600 - 0.004817 * t - 0.000014 * t2) * (dr * mv).sin();
    dl += (0.019993 - 0.000101 * t) * (dr * 2.0 * mv).sin()
        + 0.000290 * (dr * 3.0 * mv).sin();

    let mut l = (l0 + dl) * dr;
    l -= PI * 2.0 * int(l / (PI * 2.0)) as f64;
    l
}

fn get_new_moon_day(k: i64, time_zone: f64) -> i64 {
    int(new_moon(k as f64) + 0.5 + time_zone / 24.0)
}

fn get_sun_longitude(day_number: i64, time_zone: f64) -> i64 {
    int(sun_longitude(day_number as f64 - 0.5 - time_zone / 24.0) / PI * 6.0)
}

fn get_lunar_month11(yy: i64, time_zone: f64) -> i64 {
    let off = jd_from_date(31, 12, yy) - 2415021.0;
    let k = int(off / SYNODIC_MONTH);
    let mut nm = get_new_moon_day(k, time_zone);
    let sun_long = get_sun_longitude(nm, time_zone);

    if sun_long >= 9 {
        nm = get_new_moon_day(k - 1, time_zone);
    }

    nm
}

fn get_leap_month_offset(a11: i64, time_zone: f64) -> i64 {
    let k = int(0.5 + (a11 as f64 - NEW_MOON_EPOCH) / SYNODIC_MONTH);
    let mut last = 0;
    let mut i = 1;
    let mut arc =
        get_sun_longitude(get_new_moon_day(k + i, time_zone), time_zone);

    loop {
        last = arc;
        i += 1;
        arc = get_sun_longitude(get_new_moon_day(k + i, time_zone), time_zone);
        if arc == last || i >= 14 {
            break;
        }
    }

    i - 1
}

pub struct Lunar {
    pub day: i64,
    pub month: i64,
    pub year: i64,
    pub leap: bool,
}

pub fn solar_to_lunar(dd: i64, mm: i64, yy: i64, time_zone: f64) -> Lunar {
    let day_number = jd_from_date(dd, mm, yy);
    let k = int((day_number - NEW_MOON_EPOCH) / SYNODIC_MONTH);

    let mut month_start = get_new_moon_day(k + 1, time_zone);
    if month_start as f64 > day_number {
        month_start = get_new_moon_day(k, time_zone);
    }

    let mut a11 = get_lunar_month11(yy, time_zone);
    let mut b11 = a11;
    let mut lunar_year;

    if a11 >= month_start {
        lunar_year = yy;
        a11 = get_lunar_month11(yy - 1, time_zone);
    } else {
        lunar_year = yy + 1;
        b11 = get_lunar_month11(yy + 1, time_zone);
    }

    let lunar_day = day_number - month_start as f64 + 1.0;
    let diff = int((month_start - a11) as f64 / 29.0);
    let mut lunar_leap = false;
    let mut lunar_month = diff + 11;

    if b11 - a11 > 365 {
        let leap_month_diff = get_leap_month_offset(a11, time_zone);
        if diff >= leap_month_diff {
            lunar_month = diff + 10;
            if diff == leap_month_diff {
                lunar_leap = true;
            }
        }
    }

    if lunar_month > 12 {
        lunar_month -= 12;
    }

    if lunar_month >= 11 && diff < 4 {
        lunar_year -= 1;
    }

    Lunar {
        day: lunar_day as i64,
        month: lunar_month,
        year: lunar_year,
        leap: lunar_leap,
    }
}

const CAN: [&str; 10] = [
    "Giáp", "Ất", "Bính", "Đinh", "Mậu", "Kỷ", "Canh", "Tân", "Nhâm", "Quý",
];

const CHI: [&str; 12] = [
    "Tý", "Sửu", "Dần", "Mão", "Thìn", "Tỵ", "Ngọ", "Mùi", "Thân", "Dậu",
    "Tuất", "Hợi",
];

pub fn year_can_ci(year: i64) -> String {
    let can = CAN[(year + 6).rem_euclid(10) as usize];
    let chi = CHI[(year + 8).rem_euclid(12) as usize];
    format!("{can} {chi}")
}

pub fn festival(lunar: &Lunar) -> Option<&'static str> {
    if lunar.leap {
        return None;
    }

    match (lunar.day, lunar.month) {
        (1, 1) => Some("Tết Nguyên Đán"),
        (15, 1) => Some("Rằm tháng Giêng"),
        (3, 3) => Some("Tết Hàn Thực"),
        (10, 3) => Some("Giỗ Tổ Hùng Vương"),
        (15, 4) => Some("Phật Đản"),
        (5, 5) => Some("Tết Đoan Ngọ"),
        (15, 7) => Some("Lễ Vu Lan"),
        (15, 8) => Some("Tết Trung Thu"),
        (23, 12) => Some("Ông Công Ông Táo"),
        _ => None,
    }
}

pub fn days_in_solar_month(month: i64, year: i64) -> i64 {
    if month == 2 {
        let leap = year.rem_euclid(400) == 0
            || (year.rem_euclid(4) == 0 && year.rem_euclid(100) != 0);
        return if leap { 29 } else { 28 };
    }

    if month == 4 || month == 6 || month == 9 || month == 11 {
        return 30;
    }

    31
}

pub fn weekday_monday_first(dd: i64, mm: i64, yy: i64) -> i64 {
    let jd = jd_from_date(dd, mm, yy);
    (jd.rem_euclid(7.0) + 1.0) as i64
}
