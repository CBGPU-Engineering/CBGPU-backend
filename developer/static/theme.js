'use strict';
(() => {
  const media=window.matchMedia('(prefers-color-scheme: dark)');
  let choice='system';
  try{const stored=localStorage.getItem('cbgpu-developer-theme');if(['light','dark','system'].includes(stored))choice=stored}catch{}
  function apply(){const dark=choice==='dark'||choice==='system'&&media.matches;document.documentElement.dataset.theme=dark?'dark':'light';document.documentElement.style.colorScheme=dark?'dark':'light';document.querySelector('meta[name="color-scheme"]')?.setAttribute('content',dark?'dark':'light');for(const select of document.querySelectorAll('[data-theme-choice]'))select.value=choice;window.dispatchEvent(new Event('cbgpu-theme'))}
  media.addEventListener('change',apply);
  document.addEventListener('DOMContentLoaded',()=>{apply();for(const select of document.querySelectorAll('[data-theme-choice]'))select.addEventListener('change',()=>{choice=select.value;try{localStorage.setItem('cbgpu-developer-theme',choice)}catch{}apply()})});
  apply();
})();
