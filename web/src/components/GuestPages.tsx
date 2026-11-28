import React from 'react';
import { CircleDashedCheck, Users, Contact, GitBranch } from 'lucide-react';
import { msg, useT } from '../i18n';
import { BotAvatar } from './BotAvatar';
import { ExampleLabel, GuestIntro, GuestPage, OpenAuth } from './GuestWelcome';

// Approvals, Bots, Contacts and Workflows for someone who isn't signed in:
// what the page does, with a made-up example marked as one. The examples
// are fixed text, not data, so nothing here calls the server.

interface Props {
  onAuth: OpenAuth;
  bottomDock: React.ReactNode;
}

export const GuestApprovals: React.FC<Props> = ({ onAuth, bottomDock }) => {
  const t = useT();
  return (
    <GuestPage title={t('Approvals')} bottomDock={bottomDock}>
      <GuestIntro
        icon={<CircleDashedCheck className="w-6 h-6" />}
        title={t('Nothing important goes out without your OK')}
        text={t('Before a bot spends money, posts in public or messages a client, it asks here. You approve or say no in one tap.')}
        onAuth={onAuth}
      >
        <div className="glass-panel p-4 space-y-3 border-white/15" aria-hidden="true">
          <div className="flex items-center justify-between gap-2">
            <span className="flex items-center gap-2 text-[12px] text-neutral-300 min-w-0">
              <BotAvatar name="Sales Lead" size={22} />
              <span className="truncate">{t('{bot} asks', { bot: 'Sales Lead' })}</span>
            </span>
            <ExampleLabel />
          </div>
          <h3 className="text-[14px] font-semibold text-white leading-snug">
            {t('Send Dana Fox the quote for three birthday cakes: $180')}
          </h3>
          <div className="flex items-center gap-3">
            <span className="flex-1 py-2 px-4 rounded-xl text-xs text-center font-medium bg-white/[0.08] text-neutral-300 border border-white/10">
              {t('Reject…')}
            </span>
            <span className="flex-1 py-2 px-4 rounded-xl text-xs text-center font-semibold bg-accent text-accent-fg">
              {t('Approve')}
            </span>
          </div>
        </div>
      </GuestIntro>
    </GuestPage>
  );
};

/** The bots office setup starts with, by what they do. */
const STARTER_BOTS: { name: string; job: string }[] = [
  { name: 'Architect', job: msg('Hands each request to the right bot and plans the day.') },
  { name: 'Sales Lead', job: msg('Keeps your lead pipeline up to date and follows up.') },
  { name: 'Support Agent', job: msg('Answers customers on WhatsApp, Telegram or email.') },
  { name: 'Finance Manager', job: msg('Writes quotes and invoices, and checks who has paid.') },
  { name: 'Scheduler', job: msg('Books appointments into your calendar.') },
];

export const GuestBots: React.FC<Props> = ({ onAuth, bottomDock }) => {
  const t = useT();
  return (
    <GuestPage title={t('Bots')} bottomDock={bottomDock}>
      <GuestIntro
        icon={<Users className="w-6 h-6" />}
        title={t('A team that works while you do')}
        text={t('Tell the Architect what your business does and it sets up the bots you need. Each one has a job, its own room and a spending limit you choose.')}
        onAuth={onAuth}
      >
        <ul className="glass-panel divide-y divide-white/[0.06] border-white/15">
          {STARTER_BOTS.map((bot) => (
            <li key={bot.name} className="flex items-center gap-3 px-4 py-3">
              <BotAvatar name={bot.name} size={28} />
              <span className="min-w-0">
                <span className="block text-[13px] font-medium text-white">{bot.name}</span>
                <span className="block text-[12px] text-neutral-400">{t(bot.job)}</span>
              </span>
            </li>
          ))}
        </ul>
      </GuestIntro>
    </GuestPage>
  );
};

export const GuestContacts: React.FC<Props> = ({ onAuth, bottomDock }) => {
  const t = useT();
  return (
    <GuestPage title={t('Contacts')} bottomDock={bottomDock}>
      <GuestIntro
        icon={<Contact className="w-6 h-6" />}
        title={t('Everyone your business talks to, in one place')}
        text={t('Customers and leads are added from chats, your apps and Stripe, with what the bots know about each one.')}
        onAuth={onAuth}
      >
        <div className="glass-panel p-4 space-y-2 border-white/15" aria-hidden="true">
          <div className="flex items-center justify-between gap-2">
            <span className="flex items-center gap-2.5 min-w-0">
              <span className="w-8 h-8 rounded-full bg-white/10 flex items-center justify-center text-[12px] font-semibold text-white flex-shrink-0">
                DF
              </span>
              <span className="min-w-0">
                <span className="block text-[13px] font-medium text-white">Dana Fox</span>
                <span className="block text-[11px] text-neutral-500 truncate">dana@example.com</span>
              </span>
            </span>
            <ExampleLabel />
          </div>
          <p className="text-[12px] text-neutral-300 bg-white/[0.03] p-2.5 rounded-xl border border-white/5">
            {t('Ordered three birthday cakes for Saturday. Prefers WhatsApp. Paid the last invoice on time.')}
          </p>
        </div>
      </GuestIntro>
    </GuestPage>
  );
};

export const GuestWorkflows: React.FC<Props> = ({ onAuth, bottomDock }) => {
  const t = useT();
  const steps = [
    t('Confirm the order with the customer'),
    t('Add it to the pipeline'),
    t('Send the invoice when it is ready'),
  ];
  return (
    <GuestPage title={t('Workflows')} bottomDock={bottomDock}>
      <GuestIntro
        icon={<GitBranch className="w-6 h-6" />}
        title={t('Write it down once, done the same way every time')}
        text={t('A playbook is the steps for a job, like handling a new order. Bots follow it, and each run teaches them to do it better.')}
        onAuth={onAuth}
      >
        <div className="glass-panel p-4 space-y-3 border-white/15" aria-hidden="true">
          <div className="flex items-center justify-between gap-2">
            <span className="text-[13px] font-semibold text-white">{t('New order')}</span>
            <ExampleLabel />
          </div>
          <ol className="space-y-2">
            {steps.map((step, i) => (
              <li key={step} className="flex items-start gap-2.5 text-[12px] text-neutral-300">
                <span className="w-5 h-5 rounded-full bg-white/10 text-[11px] text-white flex items-center justify-center flex-shrink-0">
                  {i + 1}
                </span>
                <span className="pt-0.5">{step}</span>
              </li>
            ))}
          </ol>
        </div>
      </GuestIntro>
    </GuestPage>
  );
};
