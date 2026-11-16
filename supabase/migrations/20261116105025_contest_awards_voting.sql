-- Explicit opt-in public cards are separate from private submissions and files.
begin;
create table public.contest_award_settings(
 contest_id uuid primary key references public.contests(id) on delete cascade,
 enabled boolean not null default false,starts_at timestamptz,closes_at timestamptz,
 results_published boolean not null default false,closed_manually boolean not null default false,closed_at timestamptz,closed_period jsonb,updated_at timestamptz not null default clock_timestamp(),
 check(starts_at is null or isfinite(starts_at)),check(closes_at is null or isfinite(closes_at)),
 check(not enabled or closes_at is not null),check(starts_at is null or closes_at is null or starts_at<closes_at));
create table public.contest_award_categories(
 id uuid primary key default gen_random_uuid(),contest_id uuid not null references public.contests(id) on delete cascade,
 name text not null check(char_length(btrim(name)) between 1 and 100),description text not null default '' check(char_length(description)<=500),
 position integer not null check(position between 1 and 20),unique(contest_id,id),unique(contest_id,name),unique(contest_id,position));
create table public.contest_showcases(
 id uuid primary key default gen_random_uuid(),contest_id uuid not null,submission_user_id uuid not null,
 title text not null check(char_length(btrim(title)) between 1 and 120),summary text not null default '' check(char_length(summary)<=2000),
 demo_url text not null default '' check(demo_url='' or (char_length(demo_url)<=2000 and demo_url ~* '^https?://[^[:space:]/?#]+[^[:space:]]*$' and demo_url !~* '^https?://[^/?#]*@')),
 enabled boolean not null default true,updated_at timestamptz not null default clock_timestamp(),
 foreign key(contest_id,submission_user_id)references public.contest_submissions(contest_id,user_id)on delete cascade,
 unique(contest_id,submission_user_id),unique(contest_id,id));
create table public.contest_award_ballots(
 contest_id uuid not null,category_id uuid not null,voter_id uuid not null references auth.users(id) on delete cascade,
 showcase_id uuid not null,created_at timestamptz not null default clock_timestamp(),updated_at timestamptz not null default clock_timestamp(),
 primary key(category_id,voter_id),foreign key(contest_id,category_id)references public.contest_award_categories(contest_id,id)on delete cascade,
 foreign key(contest_id,showcase_id)references public.contest_showcases(contest_id,id)on delete cascade);
create index contest_award_ballots_tally on public.contest_award_ballots(contest_id,category_id,showcase_id);
alter table public.contest_award_settings enable row level security;
alter table public.contest_award_categories enable row level security;
alter table public.contest_showcases enable row level security;
alter table public.contest_award_ballots enable row level security;
revoke all on public.contest_award_settings,public.contest_award_categories,public.contest_showcases,public.contest_award_ballots from public,anon,authenticated;
-- No direct showcase SELECT: only the safe RPC projections below are exposed.
grant select on public.contest_award_settings,public.contest_award_categories,public.contest_award_ballots to authenticated;
create policy award_settings_read on public.contest_award_settings for select to authenticated using(true);
create policy award_categories_read on public.contest_award_categories for select to authenticated using(true);
create policy award_ballots_own_read on public.contest_award_ballots for select to authenticated using(voter_id=auth.uid());
create function contest_private.award_manager(c uuid,u uuid)returns boolean language sql stable security definer set search_path='' as $$
 select exists(select 1 from public.contests where id=c and host_id=u)or exists(select 1 from public.event_hosts where contest_id=c and user_id=u and role='manager')$$;
create function contest_private.award_participant(c uuid,u uuid)returns boolean language sql stable security definer set search_path='' as $$
 select exists(select 1 from public.event_registrations where contest_id=c and user_id=u and status='confirmed')$$;
create function contest_private.award_target_valid(c uuid,u uuid)returns boolean language sql stable security definer set search_path='' as $$
 select exists(select 1 from public.contest_submissions s join public.contests e on e.id=s.contest_id where s.contest_id=c and s.user_id=u and (
 (s.team_id is null and contest_private.award_participant(c,u))or(s.team_id is not null and exists(select 1 from public.contest_teams t where t.id=s.team_id and t.contest_id=c)
 and (select count(*) from public.contest_team_members m where m.team_id=s.team_id and contest_private.is_confirmed_member(m.team_id,m.user_id))between e.team_min_size and e.team_max_size)))$$;
revoke all on function contest_private.award_manager(uuid,uuid),contest_private.award_participant(uuid,uuid),contest_private.award_target_valid(uuid,uuid)from public,anon,authenticated;

create function public.contest_awards_api(p_contest_id uuid,p_action text,p_payload jsonb default '{}')returns jsonb
language plpgsql security definer set search_path='' as $$
declare actor uuid:=auth.uid();c public.contests;settings public.contest_award_settings;card public.contest_showcases;
 manager boolean;participant boolean;voting boolean;available boolean;kind text;item jsonb;categories jsonb;existing jsonb;normalized jsonb:='[]';category_position integer:=0;
 starts timestamptz;closes timestamptz;v_enabled boolean;v_published boolean;has_votes boolean;owner uuid;team uuid;category uuid;chosen uuid;
 v_title text;v_summary text;v_demo text;optin boolean;result jsonb;now_at timestamptz:=clock_timestamp();
begin
 if actor is null then raise exception '로그인이 필요해요.' using errcode='42501';end if;
 if p_payload is null or jsonb_typeof(p_payload)<>'object' then raise exception 'JSON 객체를 보내 주세요.';end if;
 if p_action not in('state','configure','showcase','vote','results','close')then raise exception '지원하지 않는 시상 작업입니다.';end if;
 -- Serialize config, opt-in changes and ballots for this contest; unique keys are
 -- a second, independent guard against concurrent duplicate voting.
 if p_action in('configure','showcase','vote','close')then perform pg_advisory_xact_lock(hashtextextended(p_contest_id::text,6));end if;
 select * into c from public.contests where id=p_contest_id;if not found then raise exception '대회를 찾을 수 없어요.';end if;
 manager:=contest_private.award_manager(c.id,actor);participant:=contest_private.award_participant(c.id,actor);
 select * into settings from public.contest_award_settings where contest_id=c.id;
 voting:=coalesce(settings.enabled and settings.closes_at>clock_timestamp() and(settings.starts_at is null or settings.starts_at<=clock_timestamp())and c.lifecycle_status='published',false);
 available:=manager or coalesce(settings.results_published and settings.closes_at<=clock_timestamp(),false);
 if p_action='close' then
  if not manager then raise exception '주최자 또는 운영 관리자만 투표를 마감할 수 있어요.' using errcode='42501';end if;
  if settings.contest_id is null then raise exception '시상 투표를 먼저 설정해 주세요.';end if;
  if settings.closes_at is null or settings.closes_at>clock_timestamp()then
   update public.contest_award_settings set closed_at=clock_timestamp(),closed_period=coalesce(closed_period,jsonb_build_object('starts_at',starts_at,'closes_at',closes_at)),closes_at=clock_timestamp(),starts_at=case when starts_at>=clock_timestamp()then null else starts_at end,enabled=false,results_published=false,closed_manually=true,updated_at=clock_timestamp()where contest_id=c.id;
  else
   update public.contest_award_settings set enabled=false,closed_manually=true,closed_at=coalesce(closed_at,clock_timestamp()),closed_period=coalesce(closed_period,jsonb_build_object('starts_at',starts_at,'closes_at',closes_at)),updated_at=clock_timestamp()where contest_id=c.id;
  end if;
  return public.contest_awards_api(c.id,'state','{}');
 elsif p_action='configure' then
  if not manager then raise exception '주최자 또는 운영 관리자만 시상을 설정할 수 있어요.' using errcode='42501';end if;
  v_enabled:=coalesce((p_payload->>'enabled')::boolean,settings.enabled,false);
  starts:=case when p_payload?'starts_at' then (p_payload->>'starts_at')::timestamptz else settings.starts_at end;
  closes:=case when p_payload?'closes_at' then (p_payload->>'closes_at')::timestamptz else settings.closes_at end;
  v_published:=coalesce((p_payload->>'results_published')::boolean,settings.results_published,false);
  if v_enabled and closes is null then raise exception '투표 마감 시간을 지정해 주세요.';end if;
  if(starts is not null and not isfinite(starts))or(closes is not null and not isfinite(closes))or(starts is not null and closes is not null and starts>=closes)then raise exception '투표 시작·마감 시간을 확인해 주세요.';end if;
  if v_published and(closes is null or closes>clock_timestamp())then raise exception '투표 종료 후에만 결과를 공개할 수 있어요.';end if;
  select exists(select 1 from public.contest_award_ballots where contest_id=c.id)into has_votes;
  if settings.closed_manually and(v_enabled or starts is distinct from settings.starts_at or closes is distinct from settings.closes_at)then raise exception '마감한 투표는 다시 열거나 기간을 변경할 수 없어요.';end if;
  if has_votes and(starts is distinct from settings.starts_at or closes is distinct from settings.closes_at)then raise exception '투표가 등록된 후에는 투표 기간을 변경할 수 없어요.';end if;
  if v_enabled and closes<=clock_timestamp() and closes is distinct from settings.closes_at then raise exception '새 투표 마감은 현재 서버 시각 이후로 지정해 주세요.';end if;
  categories:=p_payload->'categories';
  if categories is null and not exists(select 1 from public.contest_award_categories where contest_id=c.id)then categories:='[{"name":"Best Vibe Build","description":"가장 뛰어난 서비스"},{"name":"Best Agentic Connection","description":"서비스 간 연결과 협업"},{"name":"Most Creative Idea","description":"가장 창의적인 아이디어"},{"name":"People''s Choice","description":"참가자 인기상"}]';end if;
  if categories is not null then
   if jsonb_typeof(categories)<>'array' or jsonb_array_length(categories)not between 1 and 20 then raise exception '시상 항목은 1~20개로 설정해 주세요.';end if;
   for item in select value from jsonb_array_elements(categories)loop
    category_position:=category_position+1;category:=coalesce(nullif(item->>'id','')::uuid,gen_random_uuid());
    if jsonb_typeof(item)<>'object' or char_length(btrim(coalesce(item->>'name','')))not between 1 and 100 or char_length(coalesce(item->>'description',''))>500 then raise exception '시상 이름과 설명을 확인해 주세요.';end if;
    normalized:=normalized||jsonb_build_array(jsonb_build_object('id',category,'name',btrim(item->>'name'),'description',coalesce(item->>'description',''),'position',category_position));
   end loop;
   if(select count(distinct x->>'id')from jsonb_array_elements(normalized)x)<>category_position or(select count(distinct x->>'name')from jsonb_array_elements(normalized)x)<>category_position then raise exception '시상 항목 ID와 이름은 중복될 수 없어요.';end if;
   if exists(select 1 from public.contest_award_categories a join jsonb_array_elements(normalized)x on a.id=(x->>'id')::uuid where a.contest_id<>c.id)then raise exception '다른 대회의 시상 항목을 사용할 수 없어요.';end if;
   if has_votes or settings.results_published then
    select coalesce(jsonb_agg(jsonb_build_object('id',a.id,'name',a.name,'description',a.description,'position',a.position)order by a.position),'[]')into existing from public.contest_award_categories a where a.contest_id=c.id;
    if normalized<>existing then raise exception '투표가 등록된 후에는 시상 항목을 변경할 수 없어요.';end if;
   else
    delete from public.contest_award_categories where contest_id=c.id;
    insert into public.contest_award_categories(id,contest_id,name,description,position)select(x->>'id')::uuid,c.id,x->>'name',x->>'description',(x->>'position')::integer from jsonb_array_elements(normalized)x;
   end if;
  end if;
  insert into public.contest_award_settings(contest_id,enabled,starts_at,closes_at,results_published)values(c.id,v_enabled,starts,closes,v_published)
  on conflict(contest_id)do update set enabled=excluded.enabled,starts_at=excluded.starts_at,closes_at=excluded.closes_at,results_published=excluded.results_published,updated_at=clock_timestamp();
  return public.contest_awards_api(c.id,'state','{}');
 elsif p_action='showcase' then
  owner:=(p_payload->>'submission_user_id')::uuid;
  select s.team_id into team from public.contest_submissions s where s.contest_id=c.id and s.user_id=owner;
  if not found or(owner<>actor and(team is null or not contest_private.is_confirmed_member(team,actor)))then raise exception '본인 또는 확정된 소속 팀의 제출물만 공개할 수 있어요.' using errcode='42501';end if;
  optin:=coalesce((p_payload->>'enabled')::boolean,true);
  if optin and not contest_private.award_target_valid(c.id,owner)then raise exception '참가 확정 상태와 제출물을 확인해 주세요.';end if;
  select * into card from public.contest_showcases where contest_id=c.id and submission_user_id=owner;
  if optin and settings.closes_at is not null and settings.closes_at<=clock_timestamp()then raise exception '투표 종료 후에는 공개 작품을 변경할 수 없어요.';end if;
  if not optin then update public.contest_showcases set enabled=false,updated_at=clock_timestamp()where contest_id=c.id and submission_user_id=owner;return jsonb_build_object('ok',true,'enabled',false);end if;
  v_title:=btrim(coalesce(p_payload->>'title',''));v_summary:=coalesce(p_payload->>'summary','');v_demo:=btrim(coalesce(p_payload->>'demo_url',''));
  insert into public.contest_showcases(contest_id,submission_user_id,title,summary,demo_url,enabled)values(c.id,owner,v_title,v_summary,v_demo,true)
  on conflict(contest_id,submission_user_id)do update set title=excluded.title,summary=excluded.summary,demo_url=excluded.demo_url,enabled=true,updated_at=clock_timestamp();
  return jsonb_build_object('ok',true,'enabled',true);
 elsif p_action='vote' then
  if not voting or not participant then raise exception '투표 기간 중 참가 확정된 계정만 투표할 수 있어요.' using errcode='42501';end if;
  category:=(p_payload->>'category_id')::uuid;chosen:=(p_payload->>'showcase_id')::uuid;
  if not exists(select 1 from public.contest_award_categories where id=category and contest_id=c.id)then raise exception '시상 항목을 확인해 주세요.';end if;
  select * into card from public.contest_showcases where id=chosen and contest_id=c.id and enabled;
  if not found or not contest_private.award_target_valid(c.id,card.submission_user_id)then raise exception '공개 동의한 참가 확정 작품을 선택해 주세요.';end if;
  select s.team_id into team from public.contest_submissions s where s.contest_id=c.id and s.user_id=card.submission_user_id;
  if card.submission_user_id=actor or(team is not null and contest_private.is_confirmed_member(team,actor))then raise exception '본인 또는 소속 팀 작품에는 투표할 수 없어요.';end if;
  insert into public.contest_award_ballots(contest_id,category_id,voter_id,showcase_id)values(c.id,category,actor,chosen)
  on conflict(category_id,voter_id)do update set showcase_id=excluded.showcase_id,updated_at=clock_timestamp();
  return jsonb_build_object('ok',true,'category_id',category,'showcase_id',chosen);
 elsif p_action='results' then
  if not available then raise exception '결과는 투표 종료 후 주최자가 공개하면 확인할 수 있어요.' using errcode='42501';end if;
  with counts as(
   select a.id category_id,s.id showcase_id,case when s.enabled and contest_private.award_target_valid(c.id,s.submission_user_id)then s.title else '[Withdrawn entry]'end title,not(s.enabled and contest_private.award_target_valid(c.id,s.submission_user_id))withdrawn,count(b.voter_id)::integer votes
   from public.contest_award_categories a cross join public.contest_showcases s left join public.contest_award_ballots b on b.category_id=a.id and b.showcase_id=s.id
   where a.contest_id=c.id and s.contest_id=c.id and(coalesce(settings.closes_at<=clock_timestamp(),false)or(s.enabled and contest_private.award_target_valid(c.id,s.submission_user_id)))group by a.id,s.id,s.title
  ),ranked as(select *,max(votes)over(partition by category_id) maximum from counts),groups as(
   select a.id,a.name,a.description,a.position,coalesce(sum(r.votes),0)::integer total_votes,
    count(*)filter(where r.votes=r.maximum and r.maximum>0)>1 tied,
    coalesce(jsonb_agg(jsonb_build_object('showcase_id',r.showcase_id,'title',r.title,'withdrawn',r.withdrawn,'votes',r.votes,'is_winner',r.votes=r.maximum and r.maximum>0)order by r.votes desc,r.title,r.showcase_id)filter(where r.showcase_id is not null),'[]')entries
   from public.contest_award_categories a left join ranked r on r.category_id=a.id where a.contest_id=c.id group by a.id,a.name,a.description,a.position)
  select jsonb_build_object('categories',coalesce(jsonb_agg(jsonb_build_object('id',id,'name',name,'description',description,'total_votes',total_votes,'entries',entries,'tied',tied)order by position),'[]'),'server_time',clock_timestamp())into result from groups;
  return result;
 end if;
 -- Safe projection only. No submission answers, names, student IDs, file paths,
 -- member identities or voter identities are included in participant cards.
 return jsonb_build_object(
  'settings',jsonb_build_object('enabled',coalesce(settings.enabled,false),'starts_at',settings.starts_at,'closes_at',settings.closes_at,'results_published',coalesce(settings.results_published,false)),
  'categories',(select coalesce(jsonb_agg(jsonb_build_object('id',a.id,'name',a.name,'description',a.description,'position',a.position)order by a.position),'[]')from public.contest_award_categories a where a.contest_id=c.id),
  'showcases',(select coalesce(jsonb_agg(jsonb_build_object('id',s.id,'title',s.title,'summary',s.summary,'demo_url',s.demo_url,'is_own',s.submission_user_id=actor or(x.team_id is not null and contest_private.is_confirmed_member(x.team_id,actor)))order by s.updated_at),'[]')from public.contest_showcases s join public.contest_submissions x on x.contest_id=s.contest_id and x.user_id=s.submission_user_id where s.contest_id=c.id and s.enabled and(participant or manager)and contest_private.award_target_valid(c.id,s.submission_user_id)),
  'my_showcases',(select coalesce(jsonb_agg(jsonb_build_object('id',s.id,'submission_user_id',x.user_id,'team_id',x.team_id,'title',coalesce(s.title,''),'summary',coalesce(s.summary,''),'demo_url',coalesce(s.demo_url,''),'enabled',coalesce(s.enabled,false))),'[]')from public.contest_submissions x left join public.contest_showcases s on s.contest_id=x.contest_id and s.submission_user_id=x.user_id where x.contest_id=c.id and(x.user_id=actor or(x.team_id is not null and contest_private.is_confirmed_member(x.team_id,actor)))),
  'my_votes',(select coalesce(jsonb_agg(jsonb_build_object('category_id',b.category_id,'showcase_id',b.showcase_id)),'[]')from public.contest_award_ballots b where b.contest_id=c.id and b.voter_id=actor),
  'can_manage',manager,'can_vote',voting and participant,'results_available',available,'server_time',clock_timestamp());
end$$;
revoke all on function public.contest_awards_api(uuid,text,jsonb)from public,anon;
grant execute on function public.contest_awards_api(uuid,text,jsonb)to authenticated;
notify pgrst,'reload schema';
commit;
