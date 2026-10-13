import React from 'react';
import {ChevronLeft,ChevronRight} from 'lucide-react';
import '../../src/issues/web/pagination.js';
import '../../src/issues/web/pagination.css';

export const listPage = globalThis.HeyBossPagination.page;
export default function Pagination({page,onChange}) {
 if(page.pages<=1)return null;
 const format=value=>value.toLocaleString();
 return <div role="navigation" className="list-pagination" aria-label="Notices pagination" tabIndex={-1}>
  <span className="pagination-range">{format(page.start)}–{format(page.end)} <span>of {format(page.total)} notices</span></span>
  <div className="pagination-controls">
   <button className="page-button" disabled={page.current===1} onClick={()=>onChange(page.current-1)} aria-label="Previous page of notices"><ChevronLeft/>Previous</button>
   <span className="pagination-position" aria-live="polite">Page {format(page.current)} of {format(page.pages)}</span>
   <button className="page-button" disabled={page.current===page.pages} onClick={()=>onChange(page.current+1)} aria-label="Next page of notices">Next<ChevronRight/></button>
  </div>
 </div>;
}
