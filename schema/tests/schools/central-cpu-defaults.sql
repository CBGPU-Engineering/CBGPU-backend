-- The additive candidate must not allocate or relabel any operating project.
do $$begin
 if not exists(select 1 from cbgpu_platform_cpu.pool where id and status='DISABLED'and revision=0 and limits=cbgpu_platform_cpu.zero_limits())or exists(select 1 from cbgpu_platform_cpu.allocations)or exists(select 1 from cbgpu_platform_cpu.projects)or exists(select 1 from cbgpu_platform_cpu.leases)or exists(select 1 from cbgpu_platform_cpu.delegations)then raise exception 'CENTRAL_CPU_NOT_DEFAULT_OFF';end if;
 if exists(select 1 from public.cpu_projects where allocation_school_id is not null or deploy_actor_session_id is not null or execution_mode is not null)then raise exception 'LEGACY_CPU_PROJECT_RELABELED';end if;
end$$;
