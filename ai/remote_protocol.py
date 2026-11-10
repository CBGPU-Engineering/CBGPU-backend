"""Bounded remote occupancy-map transport and coordinate validation (no motor commands)."""
import json,math

def check(ok,message):
 if not ok:raise ValueError(message)
def finite(v):return type(v) in [int,float] and math.isfinite(v)
def grid(value):
 check(isinstance(value,dict),'Map must be an object')
 for k in ['width','height']:check(type(value.get(k)) is int and 1<=value[k]<=512,'Map dimensions must be 1–512')
 check(finite(value.get('resolution')) and .005<=value['resolution']<=5,'Invalid map resolution')
 origin=value.get('origin');check(isinstance(origin,dict) and all(finite(origin.get(k)) and abs(origin[k])<=100000 for k in ['x','y','yaw']),'Invalid map origin')
 check(isinstance(value.get('frame'),str) and 1<=len(value['frame'])<=80,'Invalid map frame')
 check(isinstance(value.get('session'),str) and 1<=len(value['session'])<=80,'Invalid mapping session')
 check(type(value.get('version')) is int and value['version']>=1,'Invalid map version')
 rle=value.get('rle');check(isinstance(rle,list) and 2<=len(rle)<=524288 and len(rle)%2==0,'Invalid map RLE');total=0
 for i in range(0,len(rle),2):
  check(type(rle[i]) is int and -1<=rle[i]<=100 and type(rle[i+1]) is int and 1<=rle[i+1]<=262144,'Invalid map cell run');total+=rle[i+1]
 check(total==value['width']*value['height'],'Map cells do not match dimensions')
 return {k:value[k] for k in ['width','height','resolution','origin','frame','session','version','rle']}
def cell(value,x,y):
 check(finite(x) and finite(y),'Coordinates must be finite');o=value['origin'];dx=x-o['x'];dy=y-o['y'];a=o['yaw'];col=math.floor((math.cos(a)*dx+math.sin(a)*dy)/value['resolution']);row=math.floor((-math.sin(a)*dx+math.cos(a)*dy)/value['resolution']);check(0<=col<value['width'] and 0<=row<value['height'],'Goal is outside the map');index=row*value['width']+col;offset=0
 for i in range(0,len(value['rle']),2):
  offset+=value['rle'][i+1]
  if index<offset:return value['rle'][i]
 raise ValueError('Invalid map index')
def free_goal(value,x,y):check(cell(value,x,y)==0,'Choose an observed free cell, not an obstacle or unknown area')
def pose(value):
 check(isinstance(value,dict) and all(finite(value.get(k)) and abs(value[k])<=100000 for k in ['x','y','yaw']),'Invalid pose');return {k:value[k] for k in ['x','y','yaw']}
