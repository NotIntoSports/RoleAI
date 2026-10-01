-- 0010（lane-R）：按用户决定移除模拟面试训练模块。
-- 0009 建立的训练表在此删除；不删除 0009 本身（已升级过的用户库版本号必须连续）。
-- session_events 保留（0009 重建过该表，其中 CHECK 白名单里的 'practice_meta'
-- 只是允许一种不会再出现的事件类型，无害；重建表做数据迁移的风险大于收益）。
DROP TABLE IF EXISTS practice_reports;
DROP TABLE IF EXISTS practice_plans;
DROP INDEX IF EXISTS idx_practice_reports_created;
