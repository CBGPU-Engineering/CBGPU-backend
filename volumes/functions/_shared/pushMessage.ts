/** Device capabilities affect delivery format only, never grant identity or resource access. */
export type PushDevice={token:string;platform:string;user_id?:string;native_kind?:string|null};
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export function buildPushMessage(device:PushDevice,title:string,body:string,androidChannelId?:string,data?:Record<string,string>){
 if(!device.token)throw Error('Device token is required');
 if(device.native_kind!=null&&device.native_kind!=='android-native-v1')throw Error('Unsupported native delivery capability');
 if(device.native_kind==='android-native-v1'){
  if(device.platform!=='android'||!device.user_id||!uuid.test(device.user_id))throw Error('Native push requires its database owner');
  // Never include notification/android.notification/webpush.notification: Android
  // otherwise renders before Kotlin can reject a signed-out or different owner.
  const nativeData={...(data||{}),owner_id:device.user_id,title:title.slice(0,120),body:body.slice(0,700),route:data?.route||data?.url||'/'};
  if(Object.values(nativeData).some(value=>typeof value!=='string'))throw Error('Native push data must contain strings');
  return {token:device.token,data:nativeData,android:{priority:'HIGH',ttl:'300s'}};
 }
 return {token:device.token,notification:{title,body},...(data?{data}:{}),...(androidChannelId?{android:{priority:'HIGH',notification:{channel_id:androidChannelId,sound:'default'}}}:{})};
}
