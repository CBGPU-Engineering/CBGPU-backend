-- Additive private storage. Apply explicitly with a database owner; never through public RPC.
create schema if not exists cbgpu_arcade;
revoke all on schema cbgpu_arcade from public, anon, authenticated;
create table cbgpu_arcade.players(user_id uuid primary key, display_name text not null default 'Player', participating boolean not null default false, check(char_length(display_name) between 2 and 24));
create table cbgpu_arcade.sessions(id uuid primary key,user_id uuid not null,game text not null,seed bigint not null,params jsonb not null,created_at timestamptz not null default now(),expires_at timestamptz not null default now()+interval '2 hours',completed_at timestamptz,submission_hash text,score bigint);
create index on cbgpu_arcade.sessions(user_id,created_at);
create index on cbgpu_arcade.sessions(game,completed_at,score desc) where completed_at is not null;
revoke all on all tables in schema cbgpu_arcade from public, anon, authenticated;
alter table cbgpu_arcade.players enable row level security;
alter table cbgpu_arcade.sessions enable row level security;
