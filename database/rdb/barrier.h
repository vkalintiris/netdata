#ifndef RDB_BARRIER_H
#define RDB_BARRIER_H

#include <condition_variable>
#include <mutex>

class Barrier
{
public:
    Barrier(int N) : N(N), Counter(0), Waiting(0) { }

    void wait()
    {
        std::unique_lock<std::mutex> L(M);

        ++Counter;
        ++Waiting;

        CV.wait(L, [&]{return Counter >= N;});
        CV.notify_one();

        --Waiting;
        if(Waiting == 0)
           Counter = 0;

        L.unlock();
    }

 private:
      std::mutex M;
      std::condition_variable CV;
      int N;
      int Counter;
      int Waiting;
};

#endif /* RDB_BARRIER_H */
