import React, {useState} from 'react';

export default function NotificationImage({attachment}) {
 const [expanded, setExpanded] = useState(false);
 const [failed, setFailed] = useState(false);
 if (!attachment || !['image/png','image/jpeg','image/gif','image/webp'].includes(attachment.mime)) return null;
 return <figure className="notification-image">
  <div className={'notification-image-canvas'+(expanded?' expanded':'')}>
   {failed ? <p role="alert">Unable to display this image.</p> : <img src={'data:'+attachment.mime+';base64,'+attachment.data} alt={attachment.name} onError={()=>setFailed(true)}/>}
  </div>
  <figcaption><span>{attachment.name}</span>{!failed&&<button type="button" aria-expanded={expanded} onClick={()=>setExpanded(!expanded)}>{expanded?'Fit image':'Full size'}</button>}</figcaption>
 </figure>;
}
