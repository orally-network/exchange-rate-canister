use ic_xrc_types::{Asset, ExchangeRateError};

use crate::utils::{median, median_in_set};
use crate::QueriedExchangeRate;

/// At least 2 stablecoin rates with respect to a third stablecoin are needed to determine if a rate is off.
pub(crate) const MIN_NUM_STABLECOIN_RATES: usize = 2;

/// Represents the errors when attempting to extract a value from JSON.
#[derive(Debug)]
pub(crate) enum StablecoinRateError {
    TooFewRates(usize),
    DifferentQuoteAssets(Asset, Asset),
    ZeroRate,
}

impl From<StablecoinRateError> for ExchangeRateError {
    fn from(error: StablecoinRateError) -> Self {
        match error {
            StablecoinRateError::TooFewRates(_) => ExchangeRateError::StablecoinRateTooFewRates,
            StablecoinRateError::DifferentQuoteAssets(_, _) => {
                ExchangeRateError::StablecoinRateNotFound
            }
            StablecoinRateError::ZeroRate => ExchangeRateError::StablecoinRateZeroRate,
        }
    }
}

impl core::fmt::Display for StablecoinRateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StablecoinRateError::TooFewRates(num_rates) => write!(
                f,
                "Not enough stablecoin rates were provided (found {})",
                num_rates
            ),
            StablecoinRateError::DifferentQuoteAssets(expected_asset, quote_asset) => write!(
                f,
                "Stablecoins provided have different quote assets (expected: {}, found: {}) ",
                expected_asset.symbol, quote_asset.symbol
            ),
            StablecoinRateError::ZeroRate => write!(f, "Calculated stablecoin rate is zero"),
        }
    }
}

/// Given a set of stablecoin exchange rates all pegged to the same target fiat currency T
/// and with the same quote asset Q but different base assets, the function determines the
/// stablecoin S that is most consistent with the other stablecoins and is therefore the best
/// approximation for the target fiat currency T and returns Q/S as an estimate for Q/T.
pub(crate) fn get_stablecoin_rate(
    stablecoin_rates: &[QueriedExchangeRate],
    target: &Asset,
) -> Result<QueriedExchangeRate, StablecoinRateError> {
    if stablecoin_rates.len() < MIN_NUM_STABLECOIN_RATES {
        return Err(StablecoinRateError::TooFewRates(stablecoin_rates.len()));
    }
    let quote_asset = &stablecoin_rates
        .get(0)
        .expect("There should always be at least one rate")
        .quote_asset;

    if let Some(rate) = stablecoin_rates
        .iter()
        .find(|rate| &rate.quote_asset != quote_asset)
    {
        return Err(StablecoinRateError::DifferentQuoteAssets(
            quote_asset.clone(),
            rate.quote_asset.clone(),
        ));
    }

    let indexed_median_rates: Vec<_> = stablecoin_rates
        .iter()
        .enumerate()
        .map(|(index, rate)| (index, median(&rate.rates)))
        .collect();

    let median_rates: Vec<_> = indexed_median_rates
        .iter()
        .map(|(_, median)| *median)
        .collect();
    // The median must exist in the set of rates.
    let median_of_median = median_in_set(&median_rates);

    if median_of_median == 0 {
        return Err(StablecoinRateError::ZeroRate);
    }

    // Retrieve the corresponding index.
    let (median_index, _) = indexed_median_rates
        .iter()
        .find(|(_, median)| *median == median_of_median)
        .expect("The stablecoin median rate must be found.");

    let median_stablecoin_rate = stablecoin_rates
        .get(*median_index)
        .expect("The stablecoin exchange rate must exist.");

    // The returned exchange rate uses the median timestamp.
    let timestamps: Vec<_> = stablecoin_rates.iter().map(|rate| rate.timestamp).collect();
    // The exchange rate canister uses timestamps without seconds.
    let median_timestamp = (median(&timestamps) / 60) * 60;

    // Construct the S/Q exchange rate struct.
    let quote_asset = Asset {
        symbol: quote_asset.symbol.clone(),
        class: quote_asset.class.clone(),
    };
    let target_to_quote_rate = QueriedExchangeRate::new(
        target.clone(),
        quote_asset,
        median_timestamp,
        &median_stablecoin_rate.rates,
        median_stablecoin_rate.base_asset_num_queried_sources,
        median_stablecoin_rate.base_asset_num_received_rates,
        None,
    );

    // Turn the S/Q rate into the Q/S = Q/T rate.
    Ok(target_to_quote_rate.inverted())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{utils, DECIMALS, RATE_UNIT};
    use ic_xrc_types::AssetClass;
    use rand::seq::SliceRandom;
    use rand::Rng;

    fn generate_stablecoin_rates(num_rates: usize, median_rate: u64) -> Vec<QueriedExchangeRate> {
        let mut rates = vec![];
        let mut initial_rates = vec![median_rate; num_rates];
        // Change less than half of the rates arbitrarily.
        let num_changed = if num_rates % 2 == 0 {
            (num_rates - 1) / 2
        } else {
            num_rates / 2
        };

        let mut rng = rand::thread_rng();
        let range: i64 = (median_rate / 10) as i64;

        for rate in initial_rates.iter_mut().take(num_changed) {
            let change: i64 = rng.gen_range(0..2 * range) - range;
            *rate = ((*rate as i64) + change) as u64;
        }
        initial_rates.shuffle(&mut rng);

        for (index, rate) in initial_rates.iter().enumerate() {
            let rate = QueriedExchangeRate::new(
                Asset {
                    symbol: ["BA", &index.to_string()].join(""),
                    class: AssetClass::Cryptocurrency,
                },
                Asset {
                    symbol: "QA".to_string(),
                    class: AssetClass::Cryptocurrency,
                },
                1647734400,
                &[*rate],
                1,
                1,
                None,
            );
            rates.push(rate);
        }
        rates
    }

    /// The function tests that the appropriate error is returned when fewer than
    /// [MIN_NUM_STABLECOIN_RATES] rates are provided.
    #[test]
    fn stablecoin_not_enough_rates() {
        let rates = generate_stablecoin_rates(1, RATE_UNIT);
        let target = Asset {
            symbol: "TA".to_string(),
            class: AssetClass::FiatCurrency,
        };

        let stablecoin_rate = get_stablecoin_rate(&rates, &target);

        assert!(matches!(
            stablecoin_rate,
            Err(StablecoinRateError::TooFewRates(1))
        ));
    }

    /// The function tests that the appropriate error is returned when there is a mismatch between
    /// quote assets.
    #[test]
    fn stablecoin_different_quote_assets() {
        let mut rates = generate_stablecoin_rates(2, RATE_UNIT);
        rates[0].quote_asset.symbol = "DA".to_string();
        let target = Asset {
            symbol: "TA".to_string(),
            class: AssetClass::FiatCurrency,
        };

        let stablecoin_rate = get_stablecoin_rate(&rates, &target);

        assert!(matches!(
            stablecoin_rate,
            Err(StablecoinRateError::DifferentQuoteAssets(_, _))
        ));
    }

    /// The function tests that the appropriate error is returned when there is a rate of zero.
    #[test]
    fn stablecoin_zero_rate() {
        let rates = generate_stablecoin_rates(2, 0);
        let target = Asset {
            symbol: "TA".to_string(),
            class: AssetClass::FiatCurrency,
        };

        let stablecoin_rate = get_stablecoin_rate(&rates, &target);

        assert!(matches!(
            stablecoin_rate,
            Err(StablecoinRateError::ZeroRate)
        ));
    }

    /// The function tests that the correct rate is returned if the majority of rates
    /// are pegged to the target currency for the case that the quote asset is also pegged.
    #[test]
    fn stablecoin_pegged_quote_asset() {
        let mut rng = rand::thread_rng();
        let num_rates = rng.gen_range(2..10);
        let rates = generate_stablecoin_rates(num_rates, RATE_UNIT);
        let target = Asset {
            symbol: "TA".to_string(),
            class: AssetClass::FiatCurrency,
        };

        let stablecoin_rate = get_stablecoin_rate(&rates, &target);

        let expected_rate = QueriedExchangeRate::new(
            rates[0].quote_asset.clone(),
            target,
            1647734400,
            &[RATE_UNIT],
            1,
            1,
            None,
        );
        assert!(matches!(stablecoin_rate, Ok(rate) if rate == expected_rate));
    }

    /// The function tests that the correct rate is returned if the majority of rates
    /// are pegged to the target currency for the case that the quote asset got depegged.
    #[test]
    fn stablecoin_depegged_quote_asset() {
        let mut rng = rand::thread_rng();
        let num_rates = rng.gen_range(2..10);
        let difference = (rng.gen_range(0..19000) as u64).saturating_sub(8500);
        let median_rate = RATE_UNIT + difference;

        let rates = generate_stablecoin_rates(num_rates, median_rate);
        let target = Asset {
            symbol: "TA".to_string(),
            class: AssetClass::FiatCurrency,
        };

        let stablecoin_rate = get_stablecoin_rate(&rates, &target);
        // The expected rate is the inverse of the median rate.
        let expected_rate = utils::checked_invert_rate(median_rate.into(), DECIMALS)
            .expect("should be able to invert the rate");
        assert!(matches!(stablecoin_rate, Ok(rate) if rate.rates[0] == expected_rate));
    }

    /// The function tests that the stablecoin with the median rate is returned.
    /// Specifically, the three stablecoins in the test have the following median rates:
    ///
    /// - median(11001, 10998, 11055, 10909) = 10999
    /// - median(9919, 9814, 10008) = 9919
    /// - median(9991, 10312, 10123, 9614, 11123) = 10123
    ///
    /// The third stablecoin has the median-of-median rate and is used as the rate of the target asset.
    #[test]
    fn stablecoin_median_of_median() {
        let first_rate = QueriedExchangeRate::new(
            Asset {
                symbol: "A".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            Asset {
                symbol: "B".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            0,
            &[11001, 10998, 11055, 10909],
            4,
            4,
            None,
        );
        let second_rate = QueriedExchangeRate::new(
            Asset {
                symbol: "C".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            Asset {
                symbol: "B".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            0,
            &[9919, 9814, 10008],
            3,
            3,
            None,
        );
        let third_rate = QueriedExchangeRate::new(
            Asset {
                symbol: "D".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            Asset {
                symbol: "B".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            0,
            &[9991, 10312, 10123, 9614, 11123],
            5,
            5,
            None,
        );
        let target_asset = Asset {
            symbol: "T".to_string(),
            class: AssetClass::FiatCurrency,
        };
        let computed_rate =
            get_stablecoin_rate(&[first_rate, second_rate, third_rate], &target_asset);
        let expected_rate = QueriedExchangeRate::new(
            Asset {
                symbol: "T".to_string(),
                class: AssetClass::FiatCurrency,
            },
            Asset {
                symbol: "B".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            0,
            &[9991, 10312, 10123, 9614, 11123],
            5,
            5,
            None,
        )
        .inverted();
        assert!(matches!(computed_rate, Ok(rate) if rate == expected_rate));
    }

    /// The function tests that a stablecoin rate is computed successfully
    /// if the number of rates is even.
    /// Specifically, the four stablecoins in the test have the following median rates:
    ///
    /// - median(11001, 10998, 11055, 10909) = 10999
    /// - median(9919, 9814, 10008) = 9919
    /// - median(9991, 10312, 10123, 9614, 11123) = 10123
    /// - median(9988, 10101) = 10044
    ///
    /// The third stablecoin has the median-of-median rate and is used as the rate of the target asset.
    #[test]
    fn stablecoin_even_number_of_rates() {
        let first_rate = QueriedExchangeRate::new(
            Asset {
                symbol: "A".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            Asset {
                symbol: "B".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            0,
            &[11001, 10998, 11055, 10909],
            4,
            4,
            None,
        );
        let second_rate = QueriedExchangeRate::new(
            Asset {
                symbol: "C".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            Asset {
                symbol: "B".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            0,
            &[9919, 9814, 10008],
            3,
            3,
            None,
        );
        let third_rate = QueriedExchangeRate::new(
            Asset {
                symbol: "D".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            Asset {
                symbol: "B".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            0,
            &[9991, 10312, 10123, 9614, 11123],
            5,
            5,
            None,
        );
        let fourth_rate = QueriedExchangeRate::new(
            Asset {
                symbol: "E".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            Asset {
                symbol: "B".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            0,
            &[9988, 10101],
            2,
            2,
            None,
        );
        let target_asset = Asset {
            symbol: "T".to_string(),
            class: AssetClass::FiatCurrency,
        };
        // The true median is 10083 and the fourth rate has the closest median at 10044,
        // so this rate is returned.
        let computed_rate = get_stablecoin_rate(
            &[first_rate, second_rate, third_rate, fourth_rate],
            &target_asset,
        );
        let expected_rate = QueriedExchangeRate::new(
            Asset {
                symbol: "T".to_string(),
                class: AssetClass::FiatCurrency,
            },
            Asset {
                symbol: "B".to_string(),
                class: AssetClass::Cryptocurrency,
            },
            0,
            &[9988, 10101],
            2,
            2,
            None,
        )
        .inverted();
        assert!(matches!(computed_rate, Ok(rate) if rate == expected_rate));
    }
}
