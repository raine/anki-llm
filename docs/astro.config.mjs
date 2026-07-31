// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import { unified } from '@astrojs/markdown-remark';
import starlightLlmsTxt from 'starlight-llms-txt';

export default defineConfig({
  site: 'https://anki-llm.raine.dev',
  markdown: {
    processor: unified({ smartypants: { dashes: false } }),
  },
  integrations: [
    starlight({
      title: 'anki-llm',
      description: 'Bulk-process and generate Anki flashcards with LLMs and text-to-speech.',
      plugins: [
        starlightLlmsTxt({
          optionalLinks: [
            {
              label: 'AnkiConnect reference',
              url: 'https://raw.githubusercontent.com/raine/anki-llm/main/ANKI_CONNECT.md',
              description: 'Low-level AnkiConnect actions and examples for agent-driven workflows.',
            },
          ],
          promote: [
            'index',
            'getting-started',
            'concepts',
            'agents',
            'process-file',
            'process-deck',
            'command-reference',
            'configuration',
          ],
          demote: [
            'prompt-reference',
            'models',
            'tts-providers',
            'ankiconnect',
            'troubleshooting',
            'faq',
          ],
          exclude: ['changelog', 'recipes/**'],
        }),
      ],
      logo: {
        dark: './src/assets/logo-dark.svg',
        light: './src/assets/logo.svg',
        alt: 'anki-llm logo',
        replacesTitle: true,
      },
      favicon: '/favicon.svg',
      head: [
        {
          tag: 'script',
          attrs: {
            src: '/image-zoom.js',
            defer: true,
          },
        },
      ],
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/raine/anki-llm' },
      ],
      components: {
        SocialIcons: './src/components/HeaderLinks.astro',
      },
      customCss: ['./src/styles/image-zoom.css'],
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'What is anki-llm?', link: '/' },
            { label: 'Getting started', slug: 'getting-started' },
            { label: 'Concepts', slug: 'concepts' },
            { label: 'Work with agents', slug: 'agents' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { label: 'Process a file', slug: 'process-file' },
            { label: 'Process a deck', slug: 'process-deck' },
            { label: 'Write prompts', slug: 'prompts' },
            { label: 'Generate cards', slug: 'generate' },
            { label: 'Processing steps', slug: 'processing-steps' },
            { label: 'Text-to-speech', slug: 'tts' },
            { label: 'Manage note types', slug: 'note-types' },
            { label: 'Use workspaces', slug: 'workspaces' },
          ],
        },
        {
          label: 'Recipes',
          items: [
            { label: 'Verify translations', slug: 'recipes/translations' },
            { label: 'Add key vocabulary', slug: 'recipes/key-vocabulary' },
            { label: 'Generate vocabulary cards', slug: 'recipes/vocabulary-cards' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Command reference', slug: 'command-reference' },
            { label: 'Prompt reference', slug: 'prompt-reference' },
            { label: 'Models', slug: 'models' },
            { label: 'TTS providers', slug: 'tts-providers' },
            { label: 'Configuration', slug: 'configuration' },
            { label: 'AnkiConnect', slug: 'ankiconnect' },
            { label: 'Troubleshooting', slug: 'troubleshooting' },
            { label: 'FAQ', slug: 'faq' },
            { label: 'Changelog', slug: 'changelog' },
          ],
        },
      ],
    }),
  ],
});
