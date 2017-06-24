
CREATE TABLE Rezonings   
(    [ID] [int] IDENTITY(1,1) NOT NULL PRIMARY KEY CLUSTERED,
     Name varchar(255) NOT NULL 
   , Status varchar(50) NOT NULL  
   , Info varchar(255) NULL  
   , SysStartTime datetime2 GENERATED ALWAYS AS ROW START NOT NULL  
   , SysEndTime datetime2 GENERATED ALWAYS AS ROW END NOT NULL  
   , PERIOD FOR SYSTEM_TIME (SysStartTime,SysEndTime)     
)    
WITH (SYSTEM_VERSIONING = ON)   
;  
insert into Rezonings (Name, Status, Info)
values ('TestName1','Approved','Infoooooo')

update Rezonings set Info = 'Infoz' where Name = '870 East 8th Avenue'

delete from Rezonings where name = '870 East 8th Avenue'

select * from Rezonings -- where status like '%x%'
order by SysStartTime desc

select * from Rezonings
where SysStartTime between '2017-06-21 20:07:56.2463138' and '9999-12-31 23:59:59.9999999'

update Rezonings set Status = 'New', Info = '' where ID = 45

select * from Rezonings
FOR SYSTEM_TIME ALL
where ID in (44,45,46)
order by ID, SysStartTime