#![cfg(test)]

mod fee_anomaly_tests {
    use anchorkit::sep38::{AnchorFeeHistory, CrossAnchorFeeAggregator};

    const NOW: u64 = 1_000_000;
    const WINDOW: u64 = 7 * 24 * 3600;

    // -------------------------------------------------------------------------
    // AnchorFeeHistory — volatility
    // -------------------------------------------------------------------------

    #[test]
    fn test_fee_volatility_none_for_single_observation() {
        let mut h = AnchorFeeHistory::new(WINDOW);
        h.record(100, 10, NOW);
        assert!(h.fee_volatility(NOW).is_none());
    }

    #[test]
    fn test_fee_volatility_zero_for_identical_fees() {
        let mut h = AnchorFeeHistory::new(WINDOW);
        h.record(100, 10, NOW - 200);
        h.record(100, 10, NOW - 100);
        h.record(100, 10, NOW);
        let vol = h.fee_volatility(NOW).unwrap();
        assert!(vol < 1e-9, "Identical fees must produce ~0 volatility, got {vol}");
    }

    #[test]
    fn test_fee_volatility_nonzero_for_varying_fees() {
        let mut h = AnchorFeeHistory::new(WINDOW);
        h.record(50, 5, NOW - 200);
        h.record(150, 5, NOW - 100);
        h.record(200, 5, NOW);
        let vol = h.fee_volatility(NOW).unwrap();
        assert!(vol > 0.0, "Varying fees must produce positive volatility");
    }

    #[test]
    fn test_fee_volatility_excludes_stale_observations() {
        let mut h = AnchorFeeHistory::new(100); // 100 s retention window
        h.record(1000, 0, NOW - 200); // outside window
        h.record(100, 0, NOW - 50);
        h.record(100, 0, NOW);
        // Only 2 active observations, both identical → volatility ≈ 0
        let vol = h.fee_volatility(NOW).unwrap();
        assert!(vol < 1e-9);
    }

    // -------------------------------------------------------------------------
    // AnchorFeeHistory — recency-weighted average
    // -------------------------------------------------------------------------

    #[test]
    fn test_recency_weighted_avg_none_for_empty() {
        let h = AnchorFeeHistory::new(WINDOW);
        assert!(h.recency_weighted_average_fee_bps(NOW).is_none());
    }

    #[test]
    fn test_recency_weighted_avg_equals_value_for_single() {
        let mut h = AnchorFeeHistory::new(WINDOW);
        h.record(200, 0, NOW);
        let avg = h.recency_weighted_average_fee_bps(NOW).unwrap();
        assert!((avg - 200.0).abs() < 1e-9);
    }

    #[test]
    fn test_recency_weighted_avg_favors_recent_spike() {
        let mut h = AnchorFeeHistory::new(WINDOW);
        // Older observations at 100 bps
        h.record(100, 0, NOW - 300);
        h.record(100, 0, NOW - 200);
        h.record(100, 0, NOW - 100);
        // Recent spike to 500 bps
        h.record(500, 0, NOW);

        let recency = h.recency_weighted_average_fee_bps(NOW).unwrap();
        let simple = h.average_fee_bps(NOW).unwrap();
        // Recency-weighted average should be higher than simple average
        assert!(
            recency > simple,
            "Recency-weighted avg ({recency}) must exceed simple avg ({simple}) when recent spike exists"
        );
    }

    // -------------------------------------------------------------------------
    // CrossAnchorFeeAggregator — compute_report (normal case)
    // -------------------------------------------------------------------------

    #[test]
    fn test_report_empty_when_no_anchors() {
        let agg = CrossAnchorFeeAggregator::new(150);
        let report = agg.compute_report(NOW);
        assert_eq!(report.median_fee_bps, 0);
        assert!(report.anomalous_anchors.is_empty());
        assert!(report.anchor_volatilities.is_empty());
    }

    #[test]
    fn test_report_no_anomalies_for_uniform_fees() {
        let mut agg = CrossAnchorFeeAggregator::new(150);
        agg.insert_observation("anchor-A", 100, NOW - 100);
        agg.insert_observation("anchor-B", 105, NOW - 50);
        agg.insert_observation("anchor-C", 102, NOW);

        let report = agg.compute_report(NOW);
        assert!(
            report.anomalous_anchors.is_empty(),
            "No anchor should be anomalous when fees are similar"
        );
    }

    #[test]
    fn test_report_flags_outlier_anchor() {
        let mut agg = CrossAnchorFeeAggregator::new(150);
        agg.insert_observation("anchor-A", 100, NOW);
        agg.insert_observation("anchor-B", 105, NOW);
        agg.insert_observation("anchor-C", 600, NOW); // large outlier

        let report = agg.compute_report(NOW);
        assert!(
            report.anomalous_anchors.iter().any(|(id, _)| id == "anchor-C"),
            "anchor-C must be flagged as anomalous"
        );
        assert!(
            !report.anomalous_anchors.iter().any(|(id, _)| id == "anchor-A"),
            "anchor-A must not be anomalous"
        );
    }

    #[test]
    fn test_report_median_is_correct_for_odd_count() {
        let mut agg = CrossAnchorFeeAggregator::new(10_000);
        agg.insert_observation("a1", 100, NOW);
        agg.insert_observation("a2", 200, NOW);
        agg.insert_observation("a3", 300, NOW);
        let report = agg.compute_report(NOW);
        assert_eq!(report.median_fee_bps, 200);
    }

    #[test]
    fn test_report_median_lower_for_even_count() {
        let mut agg = CrossAnchorFeeAggregator::new(10_000);
        agg.insert_observation("a1", 100, NOW);
        agg.insert_observation("a2", 200, NOW);
        agg.insert_observation("a3", 300, NOW);
        agg.insert_observation("a4", 400, NOW);
        // lower median for even count = fees[n/2-1] = fees[1] = 200
        let report = agg.compute_report(NOW);
        assert_eq!(report.median_fee_bps, 200);
    }

    // -------------------------------------------------------------------------
    // CrossAnchorFeeAggregator — volatility in report
    // -------------------------------------------------------------------------

    #[test]
    fn test_report_includes_volatility_for_multi_obs_anchor() {
        let mut agg = CrossAnchorFeeAggregator::new(150);
        agg.insert_observation("anchor-V", 100, NOW - 100);
        agg.insert_observation("anchor-V", 200, NOW);

        let report = agg.compute_report(NOW);
        assert!(
            report.anchor_volatilities.iter().any(|(id, _)| id == "anchor-V"),
            "anchor-V must appear in volatility list when it has multiple observations"
        );
    }

    #[test]
    fn test_report_no_volatility_for_single_obs_anchor() {
        let mut agg = CrossAnchorFeeAggregator::new(150);
        agg.insert_observation("anchor-S", 100, NOW);

        let report = agg.compute_report(NOW);
        assert!(
            !report.anchor_volatilities.iter().any(|(id, _)| id == "anchor-S"),
            "Single-observation anchor must not appear in volatility list"
        );
    }

    // -------------------------------------------------------------------------
    // CrossAnchorFeeAggregator — compute_extended_report (recency-weighted)
    // -------------------------------------------------------------------------

    #[test]
    fn test_extended_report_flags_recent_spike_as_anomalous() {
        let mut agg = CrossAnchorFeeAggregator::new(50); // tight 50 bps threshold
        // Three anchors with stable ~100 bps history
        for ts_offset in [300u64, 200, 100, 0] {
            agg.insert_observation("anchor-A", 100, NOW - ts_offset);
            agg.insert_observation("anchor-B", 105, NOW - ts_offset);
        }
        // anchor-C has a sudden recent spike to 500 bps
        agg.insert_observation("anchor-C", 100, NOW - 300);
        agg.insert_observation("anchor-C", 100, NOW - 200);
        agg.insert_observation("anchor-C", 100, NOW - 100);
        agg.insert_observation("anchor-C", 500, NOW); // recent spike

        let extended = agg.compute_extended_report(NOW);
        assert!(
            extended.anomalous_anchors.iter().any(|(id, _)| id == "anchor-C"),
            "anchor-C must be flagged by recency-weighted report due to recent spike"
        );
    }

    #[test]
    fn test_extended_report_observation_window_matches() {
        let agg = CrossAnchorFeeAggregator::new(150);
        let report = agg.compute_extended_report(NOW);
        assert_eq!(
            report.observation_window_seconds,
            CrossAnchorFeeAggregator::WINDOW_SECONDS
        );
    }
}
