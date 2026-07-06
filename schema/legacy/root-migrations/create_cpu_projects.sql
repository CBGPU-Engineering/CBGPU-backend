-- ============================================================
-- Migration: Create cpu_projects table
-- CPU 서버 배포 프로젝트 관리 테이블
-- ============================================================

CREATE TABLE IF NOT EXISTS public.cpu_projects (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  user_email TEXT NOT NULL,

  -- 프로젝트 기본 정보
  name TEXT NOT NULL,
  description TEXT DEFAULT '',

  -- 배포 방식: 'docker' (Docker Hub 이미지) | 'git' (Git 저장소)
  deploy_type TEXT NOT NULL CHECK (deploy_type IN ('docker', 'git')),

  -- Docker 모드 전용
  docker_image TEXT,                        -- 예: nginx:latest, python:3.11-slim

  -- Git 모드 전용
  git_url TEXT,                             -- 예: https://github.com/user/repo
  git_branch TEXT DEFAULT 'main',
  runtime TEXT CHECK (
    runtime IS NULL OR runtime IN ('node20', 'node18', 'python311', 'python39')
  ),
  run_command TEXT,                         -- 예: npm start, python app.py

  -- 포트 설정
  container_port INTEGER NOT NULL DEFAULT 80,  -- 컨테이너 내부 포트
  host_port INTEGER NOT NULL,                  -- 호스트 공개 포트

  -- 접근 범위: private(본인만) | school(교내망) | public(외부공개)
  access_scope TEXT NOT NULL DEFAULT 'private'
    CHECK (access_scope IN ('private', 'school', 'public')),

  -- 리소스 제한
  cpu_limit TEXT NOT NULL DEFAULT '1',        -- docker --cpus 값 (예: '0.5', '2')
  memory_limit TEXT NOT NULL DEFAULT '512m',  -- docker -m 값 (예: '512m', '1g')

  -- 환경변수 (JSON 오브젝트: {"KEY": "VALUE"})
  env_vars JSONB NOT NULL DEFAULT '{}',

  -- 라이프사이클
  -- status: pending(심사중) | approved(승인됨) | rejected(거절됨) | deleted(삭제됨)
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK (status IN ('pending', 'approved', 'rejected', 'deleted')),

  -- deploy_requested_at: 사용자가 재배포 요청 시 현재 시각으로 업데이트
  -- deployed_at:         데몬이 실제 배포 완료 후 업데이트
  -- 데몬은 deploy_requested_at > deployed_at 이면 배포(또는 재배포) 실행
  deploy_requested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  deployed_at TIMESTAMPTZ,

  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 활성 프로젝트의 host_port 중복 방지 (삭제/거절된 프로젝트는 제외)
CREATE UNIQUE INDEX IF NOT EXISTS idx_cpu_projects_host_port_active
  ON public.cpu_projects(host_port)
  WHERE status NOT IN ('deleted', 'rejected');

-- 조회 성능 인덱스
CREATE INDEX IF NOT EXISTS idx_cpu_projects_user_id ON public.cpu_projects(user_id);
CREATE INDEX IF NOT EXISTS idx_cpu_projects_status  ON public.cpu_projects(status);

-- updated_at 자동 갱신 트리거
CREATE OR REPLACE FUNCTION public.set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  NEW.updated_at = NOW();
  RETURN NEW;
END;
$$;

CREATE TRIGGER cpu_projects_set_updated_at
  BEFORE UPDATE ON public.cpu_projects
  FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();
