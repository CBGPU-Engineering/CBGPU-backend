-- ============================================================
-- Migration: Merge consecutive weekly reservations
-- Target: GPU #2, #3, #4 (weekly-only GPUs)
-- Logic: 같은 사용자, 같은 GPU, 같은 status의 연속된 주 예약을
--        하나의 예약(첫 주 월요일 ~ 마지막 주 일요일)으로 병합
-- ============================================================

-- [STEP 0] 미리보기 — 실제 변경 전 병합될 예약 확인
-- 이 쿼리만 먼저 실행해서 결과를 확인하세요.
WITH ranked AS (
  SELECT
    id,
    gpu_id,
    user_id,
    user_email,
    status,
    start_time,
    end_time,
    LAG(end_time) OVER (
      PARTITION BY gpu_id, user_id, status
      ORDER BY start_time
    ) AS prev_end
  FROM reservations
  WHERE gpu_id IN (2, 3, 4)
),
flagged AS (
  SELECT *,
    CASE
      WHEN prev_end IS NULL
        OR start_time - prev_end > INTERVAL '25 hours'
      THEN 1 ELSE 0
    END AS is_new_group
  FROM ranked
),
grouped AS (
  SELECT *,
    SUM(is_new_group) OVER (
      PARTITION BY gpu_id, user_id, status
      ORDER BY start_time
    ) AS grp
  FROM flagged
),
merged AS (
  SELECT
    (ARRAY_AGG(id ORDER BY start_time))[1] AS representative_id,
    ARRAY_AGG(id ORDER BY start_time)      AS all_ids,
    gpu_id,
    user_email,
    status,
    MIN(start_time)                        AS new_start,
    MAX(end_time)                          AS new_end,
    COUNT(*)                               AS week_count
  FROM grouped
  GROUP BY gpu_id, user_id, user_email, status, grp
  HAVING COUNT(*) > 1
)
SELECT
  representative_id,
  gpu_id,
  user_email,
  status,
  new_start,
  new_end,
  week_count,
  all_ids
FROM merged
ORDER BY gpu_id, new_start;


-- ============================================================
-- [STEP 1] 대표 행의 시간 범위를 병합된 범위로 업데이트
-- ============================================================
WITH ranked AS (
  SELECT
    id,
    gpu_id,
    user_id,
    status,
    start_time,
    end_time,
    LAG(end_time) OVER (
      PARTITION BY gpu_id, user_id, status
      ORDER BY start_time
    ) AS prev_end
  FROM reservations
  WHERE gpu_id IN (2, 3, 4)
),
flagged AS (
  SELECT *,
    CASE
      WHEN prev_end IS NULL
        OR start_time - prev_end > INTERVAL '25 hours'
      THEN 1 ELSE 0
    END AS is_new_group
  FROM ranked
),
grouped AS (
  SELECT *,
    SUM(is_new_group) OVER (
      PARTITION BY gpu_id, user_id, status
      ORDER BY start_time
    ) AS grp
  FROM flagged
),
merged AS (
  SELECT
    (ARRAY_AGG(id ORDER BY start_time))[1] AS representative_id,
    MIN(start_time)                        AS new_start,
    MAX(end_time)                          AS new_end
  FROM grouped
  GROUP BY gpu_id, user_id, status, grp
  HAVING COUNT(*) > 1
)
UPDATE reservations r
SET
  start_time = m.new_start,
  end_time   = m.new_end
FROM merged m
WHERE r.id = m.representative_id;


-- ============================================================
-- [STEP 2] 병합된 그룹에서 대표 행 외 나머지 행 삭제
-- ============================================================
WITH ranked AS (
  SELECT
    id,
    gpu_id,
    user_id,
    status,
    start_time,
    end_time,
    LAG(end_time) OVER (
      PARTITION BY gpu_id, user_id, status
      ORDER BY start_time
    ) AS prev_end
  FROM reservations
  WHERE gpu_id IN (2, 3, 4)
),
flagged AS (
  SELECT *,
    CASE
      WHEN prev_end IS NULL
        OR start_time - prev_end > INTERVAL '25 hours'
      THEN 1 ELSE 0
    END AS is_new_group
  FROM ranked
),
grouped AS (
  SELECT *,
    SUM(is_new_group) OVER (
      PARTITION BY gpu_id, user_id, status
      ORDER BY start_time
    ) AS grp
  FROM flagged
),
merged AS (
  SELECT
    (ARRAY_AGG(id ORDER BY start_time))[1] AS representative_id,
    ARRAY_AGG(id ORDER BY start_time)      AS all_ids
  FROM grouped
  GROUP BY gpu_id, user_id, status, grp
  HAVING COUNT(*) > 1
),
to_delete AS (
  SELECT UNNEST(all_ids) AS del_id, representative_id
  FROM merged
)
DELETE FROM reservations
WHERE id IN (
  SELECT del_id FROM to_delete WHERE del_id <> representative_id
);
