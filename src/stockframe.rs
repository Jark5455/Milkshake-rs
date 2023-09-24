use anyhow::{Error, Result};
use curl::easy::{Easy, List};
use libc::c_int;
use polars::export::chrono::{Duration, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use polars::prelude::{col, lit};
use polars::prelude::{
    DataFrame, DataFrameJoinOps, DataType, GroupBy, IntoLazy, JsonReader, NamedFrom, SerReader,
    Series, StrptimeOptions, TimeUnit, UniqueKeepStrategy,
};
use serde_json::Value;
use std::{env, io::Cursor, mem, ops::Sub};
use ta_lib_sys::MAType;
use ta_lib_sys::RetCode::SUCCESS;

// Helper class that constructs Dataframe for me
// in order to use it you must have alpaca api keys set as env variables
#[derive(Clone)]
pub(crate) struct StockFrame {
    pub columns: Vec<String>,
    pub tickers: Vec<String>,
    pub frame: Box<DataFrame>,
}

impl StockFrame {
    fn grab_entire_json(
        ticker: &String,
        uri: &String,
        page_token: Option<String>,
    ) -> Result<Vec<Value>> {
        let alpaca_key: String = env::var("ALPACA_KEY")?;
        let alpaca_secret: String = env::var("ALPACA_SECRET")?;

        let mut easy = Easy::new();

        let mut data = Vec::new();
        let mut headers = List::new();

        headers.append(format!("APCA-API-KEY-ID: {}", alpaca_key).as_str())?;
        headers.append(format!("APCA-API-SECRET-KEY: {}", alpaca_secret).as_str())?;

        match page_token {
            None => easy.url(uri.as_str()),
            Some(token) => easy.url(format!("{}&page_token={}", uri, token).as_str()),
        }?;

        easy.http_headers(headers)?;

        {
            let mut transfer = easy.transfer();
            transfer.write_function(|new_data| {
                data.extend_from_slice(new_data);
                Ok(new_data.len())
            })?;

            transfer.perform()?;
        }

        let json_string = String::from_utf8(data)?;
        let json_object: Value = serde_json::from_str(json_string.as_str())?;

        match json_object.get("next_page_token") {
            None => Err(Error::msg(format!("Invalid API Response: {}", json_string))),

            Some(next_page_token) => {
                let mut bars = match json_object.get("bars") {
                    None => Err(Error::msg(format!("Invalid API Response: {}", json_string))),
                    Some(bars) => match bars.as_array() {
                        None => Err(Error::msg(format!(
                            "Failed to cast bars to array: {}",
                            json_string
                        ))),
                        Some(checked_bars) => Ok(checked_bars.clone()),
                    },
                }?;

                match next_page_token.as_str() {
                    None => Ok(bars),

                    Some(page_token) => {
                        bars.append(&mut StockFrame::grab_entire_json(
                            ticker,
                            uri,
                            Some(page_token.parse()?),
                        )?);

                        Ok(bars)
                    }
                }
            }
        }
    }

    fn grab_latest_data(
        start: NaiveDateTime,
        end: NaiveDateTime,
        tickers: &Vec<String>,
    ) -> DataFrame {
        let mut df = DataFrame::default();

        for ticker in tickers {
            let grab_ticker_data = || -> Result<Cursor<String>, Error> {
                let uri = format!(
                    "https://data.alpaca.markets/v2/stocks/{}/bars?{}{}{}",
                    ticker,
                    format!(
                        "start={}&",
                        start.and_utc().to_rfc3339_opts(SecondsFormat::Secs, true)
                    ),
                    format!(
                        "end={}&",
                        end.and_utc().to_rfc3339_opts(SecondsFormat::Secs, true)
                    ),
                    "timeframe=1Min"
                );
                let entire_json = StockFrame::grab_entire_json(ticker, &uri, None)?;
                let json_tree = serde_json::to_string(&entire_json)?;
                Ok(Cursor::new(json_tree))
            };

            let cursor = match grab_ticker_data() {
                Ok(cursor) => cursor,
                Err(err) => {
                    println!(
                        "Failed to grab data for ticker: {}, Error: {}",
                        ticker,
                        err.to_string()
                    );
                    continue;
                }
            };

            let mut tmp_df = JsonReader::new(cursor)
                .finish()
                .unwrap()
                .lazy()
                .with_columns([lit(ticker.as_str()).alias("symbol")])
                .collect()
                .unwrap();

            tmp_df
                .set_column_names(
                    vec![
                        "close",
                        "high",
                        "low",
                        "trade_count",
                        "open",
                        "timestamp",
                        "volume",
                        "vwap",
                        "symbol",
                    ]
                    .as_slice(),
                )
                .expect("Collumn number mismatch");
            df = df.vstack(&tmp_df).unwrap();

            // prevent rate limiting
            std::thread::sleep(core::time::Duration::from_secs(4));
        }

        return df;
    }

    pub(crate) fn new(
        mut tickers: Option<Vec<String>>,
        mut start: Option<NaiveDateTime>,
        mut end: Option<NaiveDateTime>,
    ) -> Self {
        if tickers.is_none() {
            tickers = Some(
                vec!["AAPL", "TSLA"]
                    .iter()
                    .map(|s| String::from(*s))
                    .collect(),
            );
        }

        if start.is_none() || end.is_none() {
            end = Some(
                Utc::now()
                    .date_naive()
                    .and_hms_micro_opt(0, 0, 0, 0)
                    .unwrap(),
            );
            start = Some(end.unwrap().sub(Duration::days(30)));
        }

        assert!(tickers.is_some());

        let columns_list: Vec<String> = vec![
            "symbol",
            "timestamp",
            "open",
            "high",
            "low",
            "close",
            "volume",
            "vwap",
            "trade_count",
            "adx",
            "atr",
            "aroonosc",
            "aroonu",
            "aroond",
            "bband_up",
            "bband_mid",
            "bband_low",
            "macd",
            "macdsignal",
            "macdhist",
            "rsi",
            "stoch_slowk",
            "stoch_slowd",
            "sma",
        ]
        .iter()
        .map(|s| String::from(*s))
        .collect();
        let tickers_list = tickers.unwrap();

        let dataframe = StockFrame::grab_latest_data(start.unwrap(), end.unwrap(), &tickers_list)
            .lazy()
            .with_columns(
                columns_list[9..]
                    .iter()
                    .map(|s| lit(polars::prelude::NULL).alias(s))
                    .collect::<Vec<_>>()
                    .as_slice(),
            )
            .collect()
            .unwrap()
            .select(&columns_list)
            .unwrap()
            .to_owned();

        let dataframe_box = Box::new(dataframe);

        StockFrame {
            columns: columns_list,
            tickers: tickers_list,
            frame: dataframe_box,
        }
    }

    pub(crate) fn parse_dt_column(&mut self) {
        let lazy_df = self.frame.as_ref().clone().lazy();

        let strptimeoptions = StrptimeOptions {
            format: Some("%+".into()),
            strict: true,
            exact: false,
            cache: false,
        };

        let new_df = lazy_df
            .with_columns([col("timestamp").str().strptime(
                DataType::Datetime(TimeUnit::Milliseconds, None),
                strptimeoptions,
                lit("1970-01-01T00:00:00+00:00"),
            )])
            .collect()
            .expect("Failed to parse date time index");

        let _ = mem::replace(self.frame.as_mut(), new_df);
    }

    pub(crate) fn get_min_timestamp(&self) -> NaiveDateTime {
        let df = self.frame.as_ref();

        let dt_column = df
            .select_series(["timestamp"])
            .expect("Failed to find column named \"timestamp\"");
        let dt_series = dt_column.get(0).expect("Failed to get timestamp column");
        let ts_column: Vec<i64> = dt_series
            .datetime()
            .unwrap()
            .as_datetime_iter()
            .map(|dt| dt.unwrap().timestamp_millis())
            .collect();

        let min = *ts_column.iter().min().unwrap();
        return Utc.timestamp_opt(min / 1000, 0).unwrap().naive_utc();
    }

    pub(crate) fn get_max_timestamp(&self) -> NaiveDateTime {
        let df = self.frame.as_ref();

        let dt_column = df
            .select_series(["timestamp"])
            .expect("Failed to find column named \"timestamp\"");
        let dt_series = dt_column.get(0).expect("Failed to get timestamp column");
        let ts_column: Vec<i64> = dt_series
            .datetime()
            .unwrap()
            .as_datetime_iter()
            .map(|dt| dt.unwrap().timestamp_millis())
            .collect();

        let max = *ts_column.iter().max().unwrap();
        return Utc.timestamp_opt(max / 1000, 0).unwrap().naive_utc();
    }

    pub(crate) fn fill_date_range(&mut self) {
        let df = self.frame.as_ref();

        let max = self.get_max_timestamp().timestamp_millis();
        let mut min = self.get_min_timestamp().timestamp_millis();
        let mut date_range: Vec<i64> = vec![];

        while min <= max {
            date_range.push(min);
            min += 60000;
        }

        let ts_range: Vec<NaiveDateTime> = date_range
            .iter()
            .map(|ts| Utc.timestamp_opt(ts / 1000, 0).unwrap().naive_utc())
            .collect();
        let ts_range_series = Series::new("timestamp", ts_range);
        let new_rows = DataFrame::new(vec![ts_range_series]).unwrap().lazy();

        let lazy_df = df.clone().lazy();
        let symbol_df = lazy_df
            .select([col("symbol")])
            .unique(None, UniqueKeepStrategy::First);
        let new_index = symbol_df.cross_join(new_rows).collect().unwrap();

        let new_df = df
            .clone()
            .outer_join(&new_index, ["symbol", "timestamp"], ["symbol", "timestamp"])
            .unwrap();
        let _ = mem::replace(self.frame.as_mut(), new_df);
    }

    pub(crate) fn fill_nulls(&mut self) {
        let lazy_df = self.frame.as_ref().clone().lazy();

        let ffill_df = lazy_df
            .with_columns([col("close")
                .forward_fill(None)
                .backward_fill(None)
                .over(["symbol"])])
            .collect()
            .unwrap();

        let new_df = ffill_df
            .lazy()
            .with_columns([
                col("open").fill_null(col("close")),
                col("high").fill_null(col("close")),
                col("low").fill_null(col("close")),
            ])
            .collect()
            .unwrap();

        let _ = mem::replace(self.frame.as_mut(), new_df);
    }

    pub(crate) fn update_symbol_groups(&mut self) -> Box<GroupBy> {
        return Box::new(self.frame.group_by(["symbol"]).unwrap());
    }

    // bad TA-Lib wrapper
    pub(crate) unsafe fn calc_technical_indicators(&mut self) {
        // force sort by symbol
        let mut concat_df = DataFrame::default();
        let columns = self.columns.clone();
        let symbol_groups = self.update_symbol_groups();

        for idx in symbol_groups.get_groups().clone().iter() {
            let symbol_df = symbol_groups
                .df
                .slice(idx.first() as i64, idx.len())
                .clone();
            let high: Vec<f64> = symbol_df
                .column("high")
                .unwrap()
                .f64()
                .unwrap()
                .into_no_null_iter()
                .collect();
            let low: Vec<f64> = symbol_df
                .column("low")
                .unwrap()
                .f64()
                .unwrap()
                .into_no_null_iter()
                .collect();
            let close: Vec<f64> = symbol_df
                .column("close")
                .unwrap()
                .f64()
                .unwrap()
                .into_no_null_iter()
                .collect();

            let mut s: c_int = 0;
            let mut n: c_int = 0;

            let mut adx = vec![0f64; idx.len()];
            let mut atr = vec![0f64; idx.len()];
            let mut aroon_up = vec![0f64; idx.len()];
            let mut aroon_down = vec![0f64; idx.len()];
            let mut aroonosc = vec![0f64; idx.len()];
            let mut bband_up = vec![0f64; idx.len()];
            let mut bband_mid = vec![0f64; idx.len()];
            let mut bband_low = vec![0f64; idx.len()];
            let mut macd = vec![0f64; idx.len()];
            let mut macdsignal = vec![0f64; idx.len()];
            let mut macdhist = vec![0f64; idx.len()];
            let mut rsi = vec![0f64; idx.len()];
            let mut stoch_slowk = vec![0f64; idx.len()];
            let mut stoch_slowd = vec![0f64; idx.len()];
            let mut sma = vec![0f64; idx.len()];

            assert_eq!(
                ta_lib_sys::ADX(
                    0,
                    (close.len() - 1) as c_int,
                    high.as_ptr(),
                    low.as_ptr(),
                    close.as_ptr(),
                    14,
                    &mut s as *mut c_int,
                    &mut n as *mut c_int,
                    adx.as_mut_ptr(),
                ),
                SUCCESS
            );

            assert_eq!(
                ta_lib_sys::ATR(
                    0,
                    (close.len() - 1) as c_int,
                    high.as_ptr(),
                    low.as_ptr(),
                    close.as_ptr(),
                    14,
                    &mut s as *mut c_int,
                    &mut n as *mut c_int,
                    atr.as_mut_ptr(),
                ),
                SUCCESS
            );

            assert_eq!(
                ta_lib_sys::AROON(
                    0,
                    (close.len() - 1) as c_int,
                    high.as_ptr(),
                    low.as_ptr(),
                    14,
                    &mut s as *mut c_int,
                    &mut n as *mut c_int,
                    aroon_down.as_mut_ptr(),
                    aroon_up.as_mut_ptr(),
                ),
                SUCCESS
            );

            assert_eq!(
                ta_lib_sys::AROONOSC(
                    0,
                    (close.len() - 1) as c_int,
                    high.as_ptr(),
                    low.as_ptr(),
                    14,
                    &mut s as *mut c_int,
                    &mut n as *mut c_int,
                    aroonosc.as_mut_ptr(),
                ),
                SUCCESS
            );

            assert_eq!(
                ta_lib_sys::BBANDS(
                    0,
                    (close.len() - 1) as c_int,
                    close.as_ptr(),
                    5,
                    2f64,
                    2f64,
                    MAType::MAType_SMA,
                    &mut s as *mut c_int,
                    &mut n as *mut c_int,
                    bband_up.as_mut_ptr(),
                    bband_mid.as_mut_ptr(),
                    bband_low.as_mut_ptr(),
                ),
                SUCCESS
            );

            assert_eq!(
                ta_lib_sys::MACD(
                    0,
                    (close.len() - 1) as c_int,
                    close.as_ptr(),
                    12,
                    26,
                    9,
                    &mut s as *mut c_int,
                    &mut n as *mut c_int,
                    macd.as_mut_ptr(),
                    macdsignal.as_mut_ptr(),
                    macdhist.as_mut_ptr(),
                ),
                SUCCESS
            );

            assert_eq!(
                ta_lib_sys::RSI(
                    0,
                    (close.len() - 1) as c_int,
                    close.as_ptr(),
                    14,
                    &mut s as *mut c_int,
                    &mut n as *mut c_int,
                    rsi.as_mut_ptr(),
                ),
                SUCCESS
            );

            assert_eq!(
                ta_lib_sys::STOCH(
                    0,
                    (close.len() - 1) as c_int,
                    high.as_ptr(),
                    low.as_ptr(),
                    close.as_ptr(),
                    5,
                    3,
                    MAType::MAType_SMA,
                    3,
                    MAType::MAType_SMA,
                    &mut s as *mut c_int,
                    &mut n as *mut c_int,
                    stoch_slowk.as_mut_ptr(),
                    stoch_slowd.as_mut_ptr(),
                ),
                SUCCESS
            );

            assert_eq!(
                ta_lib_sys::SMA(
                    0,
                    (close.len() - 1) as c_int,
                    close.as_ptr(),
                    30,
                    &mut s as *mut c_int,
                    &mut n as *mut c_int,
                    sma.as_mut_ptr(),
                ),
                SUCCESS
            );

            let mut new_df = symbol_df.clone();
            new_df = new_df.drop_many(columns[9..].as_ref());

            new_df.with_column(Series::new("adx", adx.iter())).unwrap();
            new_df.with_column(Series::new("atr", atr.iter())).unwrap();
            new_df
                .with_column(Series::new("aroonosc", aroonosc.iter()))
                .unwrap();
            new_df
                .with_column(Series::new("aroonu", aroon_up.iter()))
                .unwrap();
            new_df
                .with_column(Series::new("aroond", aroon_down.iter()))
                .unwrap();
            new_df
                .with_column(Series::new("bband_up", bband_up.iter()))
                .unwrap();
            new_df
                .with_column(Series::new("bband_mid", bband_mid.iter()))
                .unwrap();
            new_df
                .with_column(Series::new("bband_low", bband_low.iter()))
                .unwrap();
            new_df
                .with_column(Series::new("macd", macd.iter()))
                .unwrap();
            new_df
                .with_column(Series::new("macdsignal", macdsignal.iter()))
                .unwrap();
            new_df
                .with_column(Series::new("macdhist", macdhist.iter()))
                .unwrap();
            new_df.with_column(Series::new("rsi", rsi.iter())).unwrap();
            new_df
                .with_column(Series::new("stoch_slowk", stoch_slowk.iter()))
                .unwrap();
            new_df
                .with_column(Series::new("stoch_slowd", stoch_slowd.iter()))
                .unwrap();
            new_df.with_column(Series::new("sma", sma.iter())).unwrap();

            concat_df = concat_df.vstack(&new_df).unwrap();
        }

        let _ = mem::replace(self.frame.as_mut(), concat_df);
    }

    // limit to trading hours (not including first 30 mins due to lack of data in that period)
    pub(crate) fn clean(&mut self) {
        let df = self.frame.as_ref().clone();
        let lazy = df.lazy();

        let new_df = lazy
            .filter(
                col("timestamp")
                    .dt()
                    .hour()
                    .lt_eq(lit(20))
                    .and(col("timestamp").dt().hour().gt_eq(lit(14))),
            )
            .collect()
            .unwrap();

        let _ = mem::replace(self.frame.as_mut(), new_df);
    }
}
