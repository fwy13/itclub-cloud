import React from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import '@fontsource/open-sans/400.css';
import '@fontsource/open-sans/600.css';
import '@fontsource/open-sans/700.css';
import '@fontsource/open-sans/800.css';
import './styles.css';

createRoot(document.getElementById('root')!).render(<React.StrictMode><App/></React.StrictMode>);
