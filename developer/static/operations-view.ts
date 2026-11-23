/** Interpret the read-only operations response without treating missing sections as empty. */
export function operationsView(value: unknown) {
  const object = value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, any> : {};
  const containers = Array.isArray(object.containers) && object.containers.every(item => item && typeof item.name === 'string' && typeof item.status === 'string' && typeof item.image === 'string') ? object.containers as {name:string;status:string;image:string}[] : null;
  const backups = Array.isArray(object.backups) && object.backups.every(item => item && typeof item.created_at === 'string' && typeof item.bytes === 'number' && Number.isFinite(item.bytes)) ? object.backups as {created_at:string;bytes:number;database?:boolean;storage?:boolean}[] : null;
  const email = object.email && typeof object.email === 'object' && !Array.isArray(object.email) ? object.email as Record<string,unknown> : null;
  return {containers, backups, email, timer:typeof object.backup_timer_active === 'boolean' ? object.backup_timer_active : null, updatedAt:typeof object.updated_at === 'string' ? object.updated_at : null, offsite:object.offsite ?? object.offsite_backup, monitor:object.monitoring ?? object.monitor, incomplete:containers === null || backups === null || email === null};
}
