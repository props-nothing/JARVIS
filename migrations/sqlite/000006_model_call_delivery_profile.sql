-- The two columns `BRN-011` needs in order to measure incremental delivery.
--
-- `docs/architecture/model-gateway.md` requires "incremental delivery as a measured
-- property, not a flag: verified time to first token *and* token spread", and
-- `docs/contracts/model-data-policy.md` records the honest state that
-- `CapabilityDescriptor::incremental_delivery` is `None` "until `BRN-011` measures
-- capabilities and an adapter attaches them". The measurement half was missing
-- entirely: `model_calls.first_output_at` gives the start of the interval and nothing
-- gave its end or its sample size, so `jarvis_domain::model::capability::IncrementalDelivery`
-- had **no producer anywhere in the workspace** and every descriptor was built with
-- `incremental_delivery: None`.
--
--   * `last_output_at` is the instant the **last** output delta of the call arrived, from
--     JARVIS's own clock. `first_output_at` alone cannot express "the output arrived in
--     one piece": a burst and a genuine stream share a first token, and the only thing
--     that separates them is when the output *finished*.
--   * `output_delta_count` is the number of `output.text.delta` frames the call produced.
--     It is the sample size of the measurement and the second half of the burst test: a
--     single delta is a burst by observation, whatever its timing.
--
-- Why three columns rather than a stored `IncrementalDelivery` blob:
--
--   * **`first_output_at` already exists and is populated**, so a blob would duplicate a
--     value that has a producer and a reader, and the two could disagree. The delivery
--     profile is *derived* from the three instants/counts at read time, which is also what
--     keeps the domain's `is_incremental()` predicate on one definition — a stored verdict
--     would be a second answer to "was this a burst", and a change to
--     `MIN_INCREMENTAL_SPREAD_MS` would silently not apply to already-written rows.
--   * **The measurement is per call, not per model.** A model's delivery profile is an
--     aggregate over its calls, which is a different quantity from one call's timing and
--     belongs wherever that aggregation is performed rather than being written into every
--     attempt row.
--
-- Both columns are nullable, and a call that produced no output records `NULL` for both rather
-- than a zero count. `NULL` means "no measurement was taken", which covers both a row written
-- before this migration and a call that delivered nothing; `0` would mean "measured, and the
-- answer is zero deltas", and a reader could not tell the two apart. The domain's own "unknown is
-- not zero" rule for the usage counters applies here for the same reason. Reporting `NULL` as
-- corruption would fail every existing database on upgrade.
--
-- The migration is purely additive, so the minimum reader stays at 1: an older binary can
-- still read a database with the two added columns.
ALTER TABLE model_calls ADD COLUMN last_output_at TEXT;
ALTER TABLE model_calls ADD COLUMN output_delta_count INTEGER;

UPDATE schema_version
SET schema_version = 6,
    min_reader_version = 1,
    writer_version = '0.1.0',
    updated_at = '1970-01-01T00:00:00Z'
WHERE id = 1;
