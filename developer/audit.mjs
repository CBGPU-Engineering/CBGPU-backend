export function auditLog(db){
 db.exec(`CREATE TABLE IF NOT EXISTS audit_events(id INTEGER PRIMARY KEY AUTOINCREMENT,at TEXT NOT NULL,actor_id TEXT,actor_email TEXT,action TEXT NOT NULL,target TEXT,outcome TEXT NOT NULL,detail TEXT); CREATE INDEX IF NOT EXISTS audit_events_time ON audit_events(at);`);
 const insert=db.prepare('INSERT INTO audit_events(at,actor_id,actor_email,action,target,outcome,detail) VALUES(?,?,?,?,?,?,?)');
 function audit(actor,action,target='',outcome='success',detail={}){insert.run(new Date().toISOString(),actor?.user_id||actor?.id||null,actor?.email||actor?.user?.email||null,String(action).slice(0,100),String(target).slice(0,300),String(outcome).slice(0,50),JSON.stringify(detail).slice(0,3000))}
 function events(limit=100){return db.prepare('SELECT * FROM audit_events ORDER BY id DESC LIMIT ?').all(Math.max(1,Math.min(500,Number(limit)||100))).map(e=>({...e,detail:JSON.parse(e.detail||'{}')}))}
 function prune(){db.prepare("DELETE FROM audit_events WHERE at<datetime('now','-90 days')").run();db.exec('DELETE FROM audit_events WHERE id<(SELECT max(id)-50000 FROM audit_events)')}
 return{audit,events,prune};
}
